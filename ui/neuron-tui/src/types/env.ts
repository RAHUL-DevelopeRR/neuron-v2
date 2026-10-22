// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: env vars
// that only fed removed features are gone. See ui/neuron-tui/PORTING.md.
/**
 * CLI-specific environment variable types.
 *
 * Extends base types from common with CLI-specific vars for:
 * - Terminal/IDE detection
 * - Editor preferences
 * - Binary build configuration
 */

import type { BaseEnv } from '@neuron-tui/common/types/contracts/env'

/**
 * CLI-specific env vars for terminal/IDE detection and editor preferences.
 */
export type CliEnv = BaseEnv & {
  // Windows system paths
  SystemRoot?: string

  // Terminal detection (for tmux/screen passthrough)
  TERM?: string
  TMUX?: string
  STY?: string

  // SSH/remote session detection
  SSH_CLIENT?: string
  SSH_TTY?: string
  SSH_CONNECTION?: string
  CODESPACES?: string
  // Cloud IDEs (reported as `cde` in the client descriptor)
  GITPOD_WORKSPACE_ID?: string
  CLOUD_SHELL?: string
  CODER?: string

  // Where Bun extracts embedded native libraries (the terminal renderer)
  BUN_TMPDIR?: string

  // Display server detection (Linux headless check)
  DISPLAY?: string
  WAYLAND_DISPLAY?: string

  // Terminal-specific
  KITTY_WINDOW_ID?: string
  SIXEL_SUPPORT?: string
  ZED_NODE_ENV?: string
  ZED_TERM?: string
  ZED_SHELL?: string
  COLORTERM?: string

  // VS Code family detection
  VSCODE_THEME_KIND?: string
  VSCODE_COLOR_THEME_KIND?: string
  VSCODE_GIT_IPC_HANDLE?: string
  VSCODE_PID?: string
  VSCODE_CWD?: string
  VSCODE_NLS_CONFIG?: string

  // Cursor editor detection
  CURSOR_PORT?: string
  CURSOR?: string

  // JetBrains IDE detection
  JETBRAINS_REMOTE_RUN?: string
  IDEA_INITIAL_DIRECTORY?: string
  IDE_CONFIG_DIR?: string
  JB_IDE_CONFIG_DIR?: string

  // Editor preferences
  VISUAL?: string
  EDITOR?: string
  NEURON_TUI_EDITOR?: string
  NEURON_EDITOR?: string

  // Theme preferences
  OPEN_TUI_THEME?: string
  OPENTUI_THEME?: string

  // Codebuff CLI-specific (set during binary build)
  NEURON_TUI_IS_BINARY?: string
  NEURON_TUI_VERSION?: string
  NEURON_TUI_TARGET?: string
  NEURON_TUI_RG_PATH?: string
  // Comma-separated registry publishers whose agents may run executable
  // handleSteps on this machine; read by the SDK (sdk/src/agent-publisher-trust.ts)
  NEURON_TUI_SCROLL_MULTIPLIER?: string
  NEURON_TUI_PERF_TEST?: string
  NEURON_TUI_LAUNCHER_PID?: string
  // Set to 1/true to suppress the terminal-reset watchdog on machines where
  // the PowerShell process shape conflicts with endpoint-security policy.
  NEURON_TUI_NO_TERMINAL_WATCHDOG?: string
  /** Absolute per-process override for isolated CLI settings and transcripts. */
  NEURON_TUI_CONFIG_DIR?: string
  /** Neuron's home, shared with the engine; the UI's files go in its `tui`. */
  NEURON_HOME?: string

  // Read only for presence/shape by the client environment descriptor
  // (utils/client-environment.ts); never sent raw.
  TZ?: string
  HTTPS_PROXY?: string
  https_proxy?: string
  HTTP_PROXY?: string
  http_proxy?: string
  ALL_PROXY?: string
  all_proxy?: string
  NODE_TLS_REJECT_UNAUTHORIZED?: string
  NODE_EXTRA_CA_CERTS?: string
}
