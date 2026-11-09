// Tests for scripts/brand-lint.mjs: node --test scripts/brand-lint.test.mjs
// (needs `bun install --frozen-lockfile` in ui/neuron-client for the
// TypeScript compiler). Leaks are planted into copies of the real files, so
// the allowlist is tested against the code it has to pass.

import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import { globToRegExp, lint, loadConfig, rustStrings, typeScriptStrings } from './brand-lint.mjs'

const SCRIPT = fileURLToPath(new URL('./brand-lint.mjs', import.meta.url))
const REPO_ROOT = path.resolve(path.dirname(SCRIPT), '..')
const config = loadConfig()

const COMMANDS = 'rust/crates/commands/src/lib.rs'
const PROMPT = 'rust/crates/runtime/src/prompt.rs'
const UI = 'ui/neuron-desktop/src/components/SignIn.tsx'
const TELEMETRY = 'rust/crates/telemetry/src/lib.rs'

function copyTree(files) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'brand-lint-'))
  for (const file of files) {
    fs.mkdirSync(path.dirname(path.join(root, file)), { recursive: true })
    fs.copyFileSync(path.join(REPO_ROOT, file), path.join(root, file))
  }
  return root
}

function edit(root, file, change) {
  const target = path.join(root, file)
  fs.writeFileSync(target, change(fs.readFileSync(target, 'utf8')))
}

const strings = (source, extract) => extract(source).map(({ start, end }) => source.slice(start, end))
const run = (...args) => spawnSync(process.execPath, [SCRIPT, ...args], { encoding: 'utf8', env: { ...process.env, GITHUB_ACTIONS: '', GITHUB_STEP_SUMMARY: '' } })

test('the current tree has no leaks outside the baseline', () => {
  const result = lint({ config })
  assert.deepEqual(result.failures, [])
  assert.ok(result.files > 100, `only ${result.files} files scanned`)
})

test('planted leaks fail, and comments, tests, model IDs and compatibility names pass', () => {
  const root = copyTree([COMMANDS, PROMPT, UI, TELEMETRY])
  try {
    // A command summary.
    let summary
    edit(root, COMMANDS, (source) => {
      const at = source.indexOf('summary: "') + 'summary: "'.length
      summary = source.slice(at, source.indexOf('"', at))
      assert.ok(summary.length > 5)
      return `${source.slice(0, at)}Ask Claude: ${source.slice(at)}
/// Claude in a doc comment
// Anthropic in a line comment
/* Claude /* nested */ still a comment */
#[cfg(test)]
mod planted_tests {
    const X: &str = "Claude in a test";
}
#[test]
fn planted_test() {
    let _ = "Anthropic in a test";
}
#[doc = "Claude in a doc attribute"]
const PLANTED_FINE: &[&str] = &["claude-opus-4-6", "CLAUDE.md", ".claude", "rusty-claude-cli", "anthropic", r#"model "claude-haiku-4-5""#];
const PLANTED_CHAR: char = '"';
const PLANTED_LIFETIME: &'static str = "plain";
const PLANTED_RAW: &str = r#"Claude says "hi""#;
`
    })
    // The system prompt.
    edit(root, PROMPT, (source) => source.replace('#[cfg(test)]', 'pub const PLANTED_INTRO: &str = "You are Claude, made by Anthropic.";\n\n#[cfg(test)]'))
    // UI text: a JSX attribute, JSX text after an apostrophe, a template literal.
    edit(
      root,
      UI,
      (source) => `${source}
export function PlantedBanner() {
  // Claude in a comment
  return <p title="Powered by Anthropic">Don't worry, Claude is here</p>
}
export const plantedTemplate = (name: string) => \`Hi \${name}, meet the Claude Code way\`
/* Anthropic in a block comment */
export const plantedFine = ['claude-sonnet-4-6', 'CLAUDE.md', '.claude/skills', 'ANTHROPIC_API_KEY', 'anthropic', /claude/i]
`,
    )
    // Request identity: the reference product's name, but not the provider's protocol.
    edit(root, TELEMETRY, (source) => `${source}\npub const PLANTED_UA: &str = "claude-cli/1.0";\npub const PLANTED_OK: [&str; 2] = ["anthropic-version", "claude-sonnet-4-6"];\n`)

    const result = lint({ root, config })
    const found = result.failures.map((hit) => `${hit.file} ${hit.term} ${hit.excerpt}`).sort()
    assert.deepEqual(found, [
      `${COMMANDS} Claude Ask Claude: ${summary}`,
      `${COMMANDS} Claude Claude says "hi"`,
      `${PROMPT} Anthropic You are Claude, made by Anthropic.`,
      `${PROMPT} Claude You are Claude, made by Anthropic.`,
      `${TELEMETRY} claude-cli claude-cli/1.0`,
      `${UI} Anthropic Powered by Anthropic`,
      `${UI} Claude Don't worry, Claude is here`,
      `${UI} Claude , meet the Claude Code way`,
    ].sort())

    // A baseline entry moves its leak from the failures to the baselined hits; one whose
    // text is gone from the file, or whose file is not in this tree, is reported as stale.
    // The entries are planted here: the real baseline only holds what is still unfixed,
    // which shrinks as leaks are fixed and can be empty.
    const knownLeak = { file: TELEMETRY, text: 'claude-cli/1.0', owner: 'test', reason: 'planted' }
    const goneText = { file: COMMANDS, text: 'text that is not in the file', owner: 'test', reason: 'planted' }
    const absentFile = { file: 'rust/crates/rusty-claude-cli/src/args.rs', text: 'a file that is not in this tree', owner: 'test', reason: 'planted' }
    const withBaseline = lint({ root, config: { ...config, baseline: [knownLeak, goneText, absentFile] } })
    assert.ok(withBaseline.baselined.some((hit) => hit.file === TELEMETRY && hit.excerpt === 'claude-cli/1.0' && hit.owner === 'test'))
    assert.ok(!withBaseline.failures.some((hit) => hit.file === TELEMETRY))
    assert.equal(withBaseline.failures.length, result.failures.length - 1)
    assert.deepEqual(withBaseline.staleBaseline, [goneText, absentFile])

    // Exit codes: new leaks fail, --report never fails.
    assert.equal(run('--root', root).status, 1)
    assert.equal(run('--root', root, '--report').status, 0)
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

test('baseline entries warn by default and fail with --strict', () => {
  const root = copyTree([PROMPT])
  try {
    // A known leak planted in the copy, with a baseline entry of its own in a copy of the allowlist.
    edit(root, PROMPT, (source) => source.replace('#[cfg(test)]', 'pub const PLANTED_KNOWN: &str = "Claude Opus 9.9";\n\n#[cfg(test)]'))
    const allowlist = JSON.parse(fs.readFileSync(path.join(path.dirname(SCRIPT), 'brand-lint.allowlist.json'), 'utf8'))
    const configFile = path.join(root, 'allowlist.json')
    fs.writeFileSync(configFile, JSON.stringify({ ...allowlist, baseline: [{ file: PROMPT, text: 'Claude Opus 9.9', owner: 'WP-test', reason: 'planted' }] }))

    const plain = run('--root', root, '--config', configFile)
    assert.equal(plain.status, 0, plain.stdout + plain.stderr)
    assert.match(plain.stdout, /prompt\.rs:\d+:\d+: warning: Claude in prompt text: "Claude Opus 9\.9" \(baseline, WP-test: planted\)/)
    assert.equal(run('--root', root, '--config', configFile, '--strict').status, 1)
    // Without the entry the same leak is new, and fails.
    assert.equal(run('--root', root).status, 1)
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

test('the Rust lexer reads strings and skips comments, doc attributes and tests', () => {
  const source = `
// "line comment"
/* "block /* nested */ comment" */
/// "doc comment"
#[doc = "doc attribute"]
#[error("error attribute")]
struct E;
fn f<'a>(x: &'a str) -> char { let _ = b"bytes"; let _ = '"'; let _ = '\\''; let _ = '\\u{1F600}'; 'x' }
const A: &str = "plain \\" escaped";
const B: &str = r##"raw "# still raw"##;
#[cfg(not(test))]
const C: &str = "not test";
#[cfg(test)]
mod tests { #[test] fn t() { let _ = "in tests"; } }
#[cfg(all(test, unix))]
fn only_in_tests() { let _ = "also tests"; }
#[tokio::test(flavor = "multi_thread")]
async fn async_test() { let _ = "async test"; }
#[cfg(test)]
use std::fmt;
const D: &str = "after the tests";
`
  assert.deepEqual(strings(source, rustStrings), ['error attribute', 'bytes', 'plain \\" escaped', 'raw "# still raw', 'not test', 'after the tests'])
})

test('the TypeScript reader takes literals and JSX text, not comments, modules or regexes', () => {
  const source = `import x from 'claude-package'
export { y } from "anthropic-package"
const lazy = () => import('dynamic-module')
// 'comment'
/* "block" */
const a = 'single', b = "double", c = \`plain template\`
const d = \`head \${a} middle \${b} tail\`
const re = /regex 'literal'/
const el = <div title="attribute">Don't {a} text</div>
`
  assert.deepEqual(strings(source, (text) => typeScriptStrings(text, 'example.tsx')), [
    'single',
    'double',
    'plain template',
    'head ',
    ' middle ',
    ' tail',
    'attribute',
    "Don't ",
    ' text',
  ])
})

test('globs', () => {
  assert.ok(globToRegExp('ui/neuron-desktop/src/**').test('ui/neuron-desktop/src/components/App.tsx'))
  assert.ok(globToRegExp('**/test/**').test('ui/neuron-desktop/test/app.test.tsx'))
  assert.ok(globToRegExp('**/*.test.*').test('a/b/c.test.ts'))
  assert.ok(!globToRegExp('ui/*/scripts/**').test('ui/neuron-tui/src/scripts.ts'))
})
