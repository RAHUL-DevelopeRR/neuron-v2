// Derived from Codebuff/Freebuff common/src/env.ts (Apache-2.0); modified for
// Neuron. See ui/neuron-tui/PORTING.md.
//
// Freebuff derived these from its validated web env schema
// (NEXT_PUBLIC_CB_ENVIRONMENT, service URLs, analytics keys). The UI only
// needs to know which kind of runtime it is in.

/** `NEURON_TUI_ENV=dev`: log to <project>/debug/cli.jsonl in real time. */
export const IS_DEV = process.env.NEURON_TUI_ENV === 'dev'

export const IS_TEST =
  process.env.NEURON_TUI_ENV === 'test' ||
  process.env.NODE_ENV === 'test' ||
  process.env.BUN_ENV === 'test'

export const IS_CI = process.env.CI === 'true' || process.env.CI === '1'
