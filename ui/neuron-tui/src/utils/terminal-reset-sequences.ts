// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the list is
// kept in sync with `neuron`'s own copy instead of the npm release launcher's,
// and MAIN_SCREEN_RESET_SEQUENCES resets a terminal already back on the main
// screen. See ui/neuron-tui/PORTING.md.
/**
 * Terminal escape sequences to reset terminal state.
 * Shared by the in-process cleanup handlers (renderer-cleanup.ts) and the
 * sacrificial watchdog process (terminal-watchdog.ts).
 *
 * Keep this list in sync with the modes OpenTUI enables (see the enable
 * sequences in @opentui/core's native lib) and with TERMINAL_RESET_SEQUENCES
 * in rust/crates/rusty-claude-cli/src/full_screen.rs, which `neuron` writes
 * after a UI that died (a test checks the two agree).
 *
 * Sequences:
 * - \x1b[?1049l: Exit alternate screen buffer (restores main screen)
 * - \x1b[?1000l: Disable X10 mouse mode
 * - \x1b[?1002l: Disable button event mouse mode
 * - \x1b[?1003l: Disable any-event mouse mode (all motion tracking)
 * - \x1b[?1006l: Disable SGR extended mouse mode
 * - \x1b[?1004l: Disable focus reporting
 * - \x1b[?2004l: Disable bracketed paste mode
 * - \x1b[<u:     Pop the kitty keyboard protocol flags OpenTUI pushes
 * - \x1b[>4;0m:  Reset xterm modifyOtherKeys (OpenTUI sets [>4;1m)
 * - \x1b[?25h:   Show cursor (safety measure)
 */
export const TERMINAL_RESET_SEQUENCES =
  '\x1b[?1049l' + // Exit alternate screen buffer
  '\x1b[?1000l' + // Disable X10 mouse mode
  '\x1b[?1002l' + // Disable button event mouse mode
  '\x1b[?1003l' + // Disable any-event mouse mode (all motion)
  '\x1b[?1006l' + // Disable SGR extended mouse mode
  '\x1b[?1004l' + // Disable focus reporting
  '\x1b[?2004l' + // Disable bracketed paste mode
  '\x1b[<u' + // Pop kitty keyboard protocol flags
  '\x1b[>4;0m' + // Reset modifyOtherKeys
  '\x1b[?25h' // Show cursor

const LEAVE_ALTERNATE_SCREEN = '\x1b[?1049l'

/**
 * The same resets for a terminal already back on the main screen. Leaving the
 * alternate screen again is not a no-op: terminals also restore the cursor
 * saved when it was entered (DECRC), which moved it up to where the UI started,
 * so the shell's next prompt overwrote the --continue hint printed below.
 */
export const MAIN_SCREEN_RESET_SEQUENCES = TERMINAL_RESET_SEQUENCES.slice(
  LEAVE_ALTERNATE_SCREEN.length,
)
