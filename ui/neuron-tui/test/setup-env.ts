// Derived from Codebuff/Freebuff sdk/test/setup-env.ts (Apache-2.0), modified
// for Neuron: only the runtime flags the UI reads; no Codebuff service env.

// Hint to the code under test that this is a test runtime.
process.env.NODE_ENV ||= 'test'
process.env.BUN_ENV ||= 'test'

if (process.env.CI !== 'true' && process.env.CI !== '1') {
  process.env.CI = 'true'
}
