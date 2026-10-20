#!/usr/bin/env bun
// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the
// Windows terminal-command broker that ran the SDK's shell tools is gone (the
// engine runs tools), so the entry point only starts the UI.
// See ui/neuron-tui/PORTING.md.

// First, so it captures PATH before the direnv import changes it.
import './engine/startup-env'

await import('./index')

export {}
