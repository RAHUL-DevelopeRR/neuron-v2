#!/usr/bin/env node
// Brand lint: Neuron's UI, help text, command summaries and system prompt must
// not name the reference product or its maker (plan guardrail 2, T6).
//
// It reads only string literals and UI text, never comments or tests:
// - TypeScript/TSX through the TypeScript compiler (string and template
//   literals, JSX text); module specifiers and regex literals are skipped.
// - Rust with a small lexer: comments, doc attributes and `#[cfg(test)]` /
//   `#[test]` items are skipped; every other string literal is read.
// - JSON string values, and HTML outside comments.
//
// What it scans, what it allows and the known leaks still being fixed live in
// scripts/brand-lint.allowlist.json:
// - `scopes`: which files, and which rule (`brand` or `product-name`). A scope
//   with `"report": true` only warns: it shows leaks someone still has to
//   decide on, without failing the run. A file belongs to the first scope
//   that includes it.
// - `allow`: patterns masked out before matching (model IDs, compatibility
//   paths and names, internal crate names). `exact`: whole literals allowed.
// - `accepted`: one literal in one file, allowed with a reason.
// - `baseline`: known leaks with an owner. They are reported as warnings and
//   do not fail the run; anything else fails it. A baseline entry that no
//   longer matches is reported so it can be deleted.
//
// Usage: node scripts/brand-lint.mjs [--report] [--strict] [--json] [--root DIR] [--config FILE]
//   --report  print everything, always exit 0
//   --strict  treat baseline entries as failures too
//   --json    print the result as JSON
// The TypeScript compiler comes from a UI package's node_modules: run
// `bun install --frozen-lockfile` in ui/neuron-client first.

import fs from 'node:fs'
import path from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath, pathToFileURL } from 'node:url'

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url))
const REPO_ROOT = path.resolve(SCRIPT_DIR, '..')
const DEFAULT_CONFIG = path.join(SCRIPT_DIR, 'brand-lint.allowlist.json')
const SKIP_DIRS = new Set(['node_modules', 'target', 'dist', '.git', 'gen', '.expo'])
const SOURCE_EXTENSIONS = new Set(['.ts', '.tsx', '.mts', '.cts', '.js', '.jsx', '.mjs', '.cjs', '.rs', '.json', '.html'])

export const RULES = {
  // Any mention of the reference product or its maker.
  brand: /claude|anthropic/gi,
  // The reference products' own names, for request identity (app name, beta
  // headers): model IDs and the provider's protocol names are fine there.
  'product-name': /claude[\s_-]?(?:code|cli|desktop)/gi,
}

// ---------------------------------------------------------------------------
// Configuration

export function loadConfig(configPath = DEFAULT_CONFIG) {
  const raw = JSON.parse(fs.readFileSync(configPath, 'utf8'))
  const compile = (entry) => new RegExp(entry.pattern, entry.flags?.includes('g') ? entry.flags : `${entry.flags ?? ''}g`)
  for (const scope of raw.scopes) {
    if (!RULES[scope.rule]) throw new Error(`brand-lint: scope ${scope.name} has unknown rule ${scope.rule}`)
  }
  return {
    scopes: raw.scopes.map((scope) => ({ ...scope, include: scope.include.map(globToRegExp) })),
    exclude: raw.exclude.map(globToRegExp),
    allow: raw.allow.map((entry) => ({ ...entry, regex: compile(entry) })),
    exact: new Set(raw.exact.map((entry) => entry.text)),
    accepted: raw.accepted,
    baseline: raw.baseline,
  }
}

export function globToRegExp(glob) {
  let out = ''
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i]
    if (c === '*' && glob[i + 1] === '*') {
      // `**/` matches zero or more directories; a trailing `**` matches the rest.
      if (glob[i + 2] === '/') {
        out += '(?:.*/)?'
        i += 2
      } else {
        out += '.*'
        i += 1
      }
    } else if (c === '*') {
      out += '[^/]*'
    } else if (c === '?') {
      out += '[^/]'
    } else {
      out += c.replace(/[.+^${}()|[\]\\]/g, '\\$&')
    }
  }
  return new RegExp(`^${out}$`)
}

// ---------------------------------------------------------------------------
// String extraction. Each extractor returns [{ start, end }] offsets of the
// literal's content (without its delimiters) in the source.

let typescript
function loadTypeScript() {
  if (typescript) return typescript
  for (const pkg of ['ui/neuron-client', 'ui/neuron-desktop', 'ui/neuron-mobile', 'ui/neuron-tui']) {
    try {
      typescript = createRequire(path.join(REPO_ROOT, pkg, 'package.json'))('typescript')
      return typescript
    } catch {
      // try the next package
    }
  }
  throw new Error('brand-lint needs the TypeScript compiler: run `bun install --frozen-lockfile` in ui/neuron-client first.')
}

export function typeScriptStrings(source, fileName) {
  const ts = loadTypeScript()
  const ext = path.extname(fileName)
  const kind = ext === '.tsx' ? ts.ScriptKind.TSX : ext === '.jsx' ? ts.ScriptKind.JSX : ext.endsWith('ts') ? ts.ScriptKind.TS : ts.ScriptKind.JS
  const file = ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, true, kind)
  const K = ts.SyntaxKind
  const out = []
  const visit = (node) => {
    switch (node.kind) {
      // Module specifiers name packages, not anything a user reads.
      case K.ImportDeclaration:
      case K.ExportDeclaration:
      case K.ImportType:
      case K.ExternalModuleReference:
        return
      case K.CallExpression:
        if (node.expression.kind === K.ImportKeyword || (node.expression.kind === K.Identifier && node.expression.text === 'require')) return
        break
      case K.StringLiteral:
      case K.NoSubstitutionTemplateLiteral:
        out.push({ start: node.getStart(file) + 1, end: node.end - 1 })
        return
      case K.TemplateHead:
      case K.TemplateMiddle:
        out.push({ start: node.getStart(file) + 1, end: node.end - 2 })
        return
      case K.TemplateTail:
        out.push({ start: node.getStart(file) + 1, end: node.end - 1 })
        return
      case K.JsxText:
        if (!node.containsOnlyTriviaWhiteSpaces) out.push({ start: node.pos, end: node.end })
        return
    }
    ts.forEachChild(node, visit)
  }
  visit(file)
  return out
}

const isIdentChar = (c) => c !== undefined && /[A-Za-z0-9_]/.test(c)

// The end (exclusive) of the bracketed group opening at `open`, skipping
// string literals inside it.
function bracketEnd(source, open) {
  let depth = 0
  for (let i = open; i < source.length; i++) {
    const c = source[i]
    if (c === '"') {
      i++
      while (i < source.length && source[i] !== '"') i += source[i] === '\\' ? 2 : 1
    } else if (c === '[') {
      depth++
    } else if (c === ']') {
      depth--
      if (depth === 0) return i + 1
    }
  }
  return source.length
}

// `#[test]`, `#[tokio::test(…)]`, `#[cfg(test)]`, `#[cfg(all(test, …))]`; not `#[cfg(not(test))]`.
const TEST_ATTRIBUTE = /^#!?\[\s*(?:(?:\w+::)?test\s*[\](]|cfg\s*\((?![^\]]*\bnot\s*\()[^\]]*\btest\b)/
const DOC_ATTRIBUTE = /^#!?\[\s*doc\b/

export function rustStrings(source) {
  const out = []
  const n = source.length
  let depth = 0
  let skipToDepth = -1 // inside a test item: strings are ignored until depth returns here
  let pendingSkip = false // a test attribute was seen; the next item is skipped
  const record = (start, end) => {
    if (skipToDepth < 0 && !pendingSkip) out.push({ start, end })
  }
  let i = 0
  while (i < n) {
    const c = source[i]
    const next = source[i + 1]
    if (c === '/' && next === '/') {
      const eol = source.indexOf('\n', i)
      i = eol < 0 ? n : eol
      continue
    }
    if (c === '/' && next === '*') {
      let level = 1
      i += 2
      while (i < n && level > 0) {
        if (source[i] === '/' && source[i + 1] === '*') {
          level++
          i += 2
        } else if (source[i] === '*' && source[i + 1] === '/') {
          level--
          i += 2
        } else {
          i++
        }
      }
      continue
    }
    // Raw strings: r"…", r#"…"#, br"…", cr"…" (no escapes inside).
    if ((c === 'r' || ((c === 'b' || c === 'c') && next === 'r')) && !isIdentChar(source[i - 1])) {
      let j = i + (c === 'r' ? 1 : 2)
      let hashes = 0
      while (source[j] === '#') {
        hashes++
        j++
      }
      if (source[j] === '"') {
        const close = `"${'#'.repeat(hashes)}`
        const found = source.indexOf(close, j + 1)
        const end = found < 0 ? n : found
        record(j + 1, end)
        i = end + close.length
        continue
      }
    }
    if (c === '"') {
      let j = i + 1
      while (j < n && source[j] !== '"') j += source[j] === '\\' ? 2 : 1
      record(i + 1, j)
      i = j + 1
      continue
    }
    if (c === "'") {
      // A char literal ('x', '\n', '\u{..}', '"') or a lifetime ('a).
      if (next === '\\') {
        let j = i + 3
        while (j < n && source[j] !== "'") j++
        i = j + 1
        continue
      }
      const width = (source.codePointAt(i + 1) ?? 0) > 0xffff ? 2 : 1
      if (source[i + 1 + width] === "'") {
        i += 2 + width
        continue
      }
      i++
      continue
    }
    if (c === '#' && (next === '[' || (next === '!' && source[i + 2] === '['))) {
      const end = bracketEnd(source, source.indexOf('[', i))
      const attribute = source.slice(i, end)
      if (DOC_ATTRIBUTE.test(attribute)) {
        i = end
        continue
      }
      if (TEST_ATTRIBUTE.test(attribute)) {
        if (next === '!') return [] // the whole file is test-only
        if (skipToDepth < 0) pendingSkip = true
        i = end
        continue
      }
      i += next === '!' ? 3 : 2 // read the attribute's strings (e.g. error messages)
      continue
    }
    if (c === '{') {
      depth++
      if (pendingSkip) {
        skipToDepth = depth - 1
        pendingSkip = false
      }
    } else if (c === '}') {
      depth--
      if (skipToDepth >= 0 && depth === skipToDepth) skipToDepth = -1
    } else if (c === ';' && pendingSkip && depth >= 0) {
      pendingSkip = false // `#[cfg(test)] use …;` or `mod tests;`
    }
    i++
  }
  return out
}

export function jsonStrings(source) {
  const out = []
  const re = /"((?:[^"\\]|\\.)*)"/g
  for (const match of source.matchAll(re)) out.push({ start: match.index + 1, end: match.index + match[0].length - 1 })
  return out
}

export function htmlStrings(source) {
  // The whole page outside comments is user-visible or markup.
  const out = []
  let last = 0
  for (const match of source.matchAll(/<!--[\s\S]*?-->/g)) {
    out.push({ start: last, end: match.index })
    last = match.index + match[0].length
  }
  out.push({ start: last, end: source.length })
  return out
}

export function extractStrings(source, fileName) {
  switch (path.extname(fileName)) {
    case '.rs':
      return rustStrings(source)
    case '.json':
      return jsonStrings(source)
    case '.html':
      return htmlStrings(source)
    default:
      return typeScriptStrings(source, fileName)
  }
}

// ---------------------------------------------------------------------------
// Matching

function lineAndColumn(source, offset) {
  let line = 1
  let lineStart = 0
  for (let i = source.indexOf('\n'); i >= 0 && i < offset; i = source.indexOf('\n', i + 1)) {
    line++
    lineStart = i + 1
  }
  return { line, column: offset - lineStart + 1 }
}

export function findTerms(content, rule, config) {
  if (config.exact.has(content.trim())) return []
  let masked = content
  for (const entry of config.allow) masked = masked.replace(entry.regex, (m) => ' '.repeat(m.length))
  const re = new RegExp(RULES[rule].source, RULES[rule].flags)
  return [...masked.matchAll(re)].map((m) => ({ index: m.index, term: content.slice(m.index, m.index + m[0].length) }))
}

const matchesEntry = (entry, hit) => entry.file === hit.file && hit.literal.includes(entry.text)

function excerpt(literal, index) {
  const flat = literal.replace(/\s+/g, ' ')
  if (flat.length <= 100) return flat.trim()
  const offset = literal.slice(0, index).replace(/\s+/g, ' ').length
  const start = Math.max(0, offset - 45)
  return `${start > 0 ? '…' : ''}${flat.slice(start, start + 100).trim()}${start + 100 < flat.length ? '…' : ''}`
}

function* walk(root, dir) {
  let entries
  try {
    entries = fs.readdirSync(path.join(root, dir), { withFileTypes: true })
  } catch {
    return
  }
  for (const entry of entries) {
    const rel = dir ? `${dir}/${entry.name}` : entry.name
    if (entry.isDirectory()) {
      if (!SKIP_DIRS.has(entry.name)) yield* walk(root, rel)
    } else if (entry.isFile() && SOURCE_EXTENSIONS.has(path.extname(entry.name))) {
      yield rel
    }
  }
}

// The fixed directory prefix of a glob, so only that part of the tree is walked.
function globBase(regex) {
  const literal = regex.source.slice(1).split(/\[\^\/\]|\(\?:|\.\*/)[0]
  const base = literal.replace(/\\(.)/g, '$1')
  return base.includes('/') ? base.slice(0, base.lastIndexOf('/')) : ''
}

export function lint({ root = REPO_ROOT, config = loadConfig() } = {}) {
  const files = new Map() // file -> scope
  for (const scope of config.scopes) {
    for (const include of scope.include) {
      for (const file of walk(root, globBase(include))) {
        if (include.test(file) && !config.exclude.some((ex) => ex.test(file)) && !files.has(file)) files.set(file, scope)
      }
    }
  }

  const hits = []
  for (const [file, scope] of [...files].sort(([a], [b]) => a.localeCompare(b))) {
    const source = fs.readFileSync(path.join(root, file), 'utf8')
    for (const { start, end } of extractStrings(source, file)) {
      const literal = source.slice(start, end)
      for (const { index, term } of findTerms(literal, scope.rule, config)) {
        hits.push({ file, scope: scope.name, report: Boolean(scope.report), ...lineAndColumn(source, start + index), term, literal, excerpt: excerpt(literal, index) })
      }
    }
  }

  const result = { files: files.size, failures: [], baselined: [], reported: [], accepted: [], staleBaseline: [], staleAccepted: [] }
  for (const hit of hits) {
    const { literal, report, ...shown } = hit
    const accepted = config.accepted.find((entry) => matchesEntry(entry, hit))
    const baseline = config.baseline.find((entry) => matchesEntry(entry, hit))
    if (accepted) result.accepted.push({ ...shown, reason: accepted.reason })
    else if (baseline) result.baselined.push({ ...shown, owner: baseline.owner, reason: baseline.reason })
    else if (report) result.reported.push(shown)
    else result.failures.push(shown)
  }
  result.staleBaseline = config.baseline.filter((entry) => !hits.some((hit) => matchesEntry(entry, hit)))
  result.staleAccepted = config.accepted.filter((entry) => !hits.some((hit) => matchesEntry(entry, hit)))
  return result
}

// ---------------------------------------------------------------------------
// Command line

function report(result, { strict, reportOnly }) {
  const annotate = process.env.GITHUB_ACTIONS === 'true'
  const failing = strict ? [...result.failures, ...result.baselined] : result.failures
  const print = (level, hit, note) => {
    const message = `${hit.term} in ${hit.scope} text: "${hit.excerpt}"${note ? ` (${note})` : ''}`
    console.log(`${hit.file}:${hit.line}:${hit.column}: ${level}: ${message}`)
    if (annotate) console.log(`::${level} file=${hit.file},line=${hit.line},col=${hit.column},title=brand-lint::${message.replace(/%/g, '%25').replace(/\r?\n/g, '%0A')}`)
  }
  for (const hit of result.failures) print(reportOnly ? 'warning' : 'error', hit)
  for (const hit of result.baselined) print(strict && !reportOnly ? 'error' : 'warning', hit, `baseline, ${hit.owner}: ${hit.reason}`)
  for (const hit of result.reported) print('warning', hit, 'report-only scope')
  for (const entry of result.staleBaseline) console.log(`${entry.file}: notice: baseline entry "${entry.text}" no longer matches; delete it from brand-lint.allowlist.json`)
  for (const entry of result.staleAccepted) console.log(`${entry.file}: notice: accepted entry "${entry.text}" no longer matches; delete it from brand-lint.allowlist.json`)

  const summary = `brand-lint: ${result.files} files, ${result.failures.length} new, ${result.baselined.length} baselined, ${result.reported.length} in report-only scopes, ${result.accepted.length} accepted`
  console.log(summary)
  if (process.env.GITHUB_STEP_SUMMARY) {
    const rows = [
      ...result.failures.map((hit) => ['new', hit]),
      ...result.baselined.map((hit) => [`baseline (${hit.owner})`, hit]),
      ...result.reported.map((hit) => [`report only (${hit.scope})`, hit]),
    ]
    const table = rows.length
      ? ['| Status | Where | Text |', '|---|---|---|', ...rows.map(([status, hit]) => `| ${status} | \`${hit.file}:${hit.line}\` | ${hit.excerpt.replace(/\|/g, '\\|')} |`)].join('\n')
      : 'No brand leaks.'
    const mode = reportOnly ? 'report mode: nothing fails' : strict ? 'strict mode: baseline entries fail too' : 'new leaks fail; baseline entries are warnings'
    fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, `## Brand lint\n\n${summary} (${mode}).\n\n${table}\n`)
  }
  return failing.length > 0 && !reportOnly ? 1 : 0
}

function main(argv) {
  const args = { root: REPO_ROOT, config: DEFAULT_CONFIG, strict: false, reportOnly: false, json: false }
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]
    if (arg === '--report') args.reportOnly = true
    else if (arg === '--strict') args.strict = true
    else if (arg === '--json') args.json = true
    else if (arg === '--root') args.root = path.resolve(argv[++i])
    else if (arg === '--config') args.config = path.resolve(argv[++i])
    else {
      console.error(`brand-lint: unknown argument ${arg}`)
      return 2
    }
  }
  const result = lint({ root: args.root, config: loadConfig(args.config) })
  if (args.json) {
    console.log(JSON.stringify(result, null, 2))
    return result.failures.length > 0 && !args.reportOnly ? 1 : 0
  }
  return report(result, args)
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    process.exitCode = main(process.argv.slice(2))
  } catch (error) {
    console.error(error.message)
    process.exitCode = 2
  }
}
