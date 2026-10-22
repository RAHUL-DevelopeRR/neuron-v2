// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: env vars
// that only fed removed features are gone. See ui/neuron-tui/PORTING.md.
/**
 * CLI environment helper for dependency injection.
 *
 * This module provides CLI-specific env helpers that extend the base
 * process env with CLI-specific vars for terminal/IDE detection.
 */

import { getBaseEnv } from '@neuron-tui/common/env-process'

import type { CliEnv } from '../types/env'

/**
 * Get CLI environment values.
 * Composes from getBaseEnv() + CLI-specific vars.
 */
export const getCliEnv = (): CliEnv => ({
  ...getBaseEnv(),

  // Windows system paths
  SystemRoot: process.env.SystemRoot,

  // Display server detection (Linux headless check)
  DISPLAY: process.env.DISPLAY,
  WAYLAND_DISPLAY: process.env.WAYLAND_DISPLAY,

  // Terminal detection (for tmux/screen passthrough)
  TERM: process.env.TERM,
  TMUX: process.env.TMUX,
  STY: process.env.STY,

  // SSH/remote session detection
  SSH_CLIENT: process.env.SSH_CLIENT,
  SSH_TTY: process.env.SSH_TTY,
  SSH_CONNECTION: process.env.SSH_CONNECTION,
  CODESPACES: process.env.CODESPACES,
  GITPOD_WORKSPACE_ID: process.env.GITPOD_WORKSPACE_ID,
  CLOUD_SHELL: process.env.CLOUD_SHELL,
  CODER: process.env.CODER,

  // Where Bun extracts embedded native libraries (the terminal renderer)
  BUN_TMPDIR: process.env.BUN_TMPDIR,

  // Terminal detection
  KITTY_WINDOW_ID: process.env.KITTY_WINDOW_ID,
  SIXEL_SUPPORT: process.env.SIXEL_SUPPORT,
  ZED_NODE_ENV: process.env.ZED_NODE_ENV,
  ZED_TERM: process.env.ZED_TERM,
  ZED_SHELL: process.env.ZED_SHELL,
  COLORTERM: process.env.COLORTERM,

  // VS Code family detection
  VSCODE_THEME_KIND: process.env.VSCODE_THEME_KIND,
  VSCODE_COLOR_THEME_KIND: process.env.VSCODE_COLOR_THEME_KIND,
  VSCODE_GIT_IPC_HANDLE: process.env.VSCODE_GIT_IPC_HANDLE,
  VSCODE_PID: process.env.VSCODE_PID,
  VSCODE_CWD: process.env.VSCODE_CWD,
  VSCODE_NLS_CONFIG: process.env.VSCODE_NLS_CONFIG,

  // Cursor editor detection
  CURSOR_PORT: process.env.CURSOR_PORT,
  CURSOR: process.env.CURSOR,

  // JetBrains IDE detection
  JETBRAINS_REMOTE_RUN: process.env.JETBRAINS_REMOTE_RUN,
  IDEA_INITIAL_DIRECTORY: process.env.IDEA_INITIAL_DIRECTORY,
  IDE_CONFIG_DIR: process.env.IDE_CONFIG_DIR,
  JB_IDE_CONFIG_DIR: process.env.JB_IDE_CONFIG_DIR,

  // Editor preferences
  VISUAL: process.env.VISUAL,
  EDITOR: process.env.EDITOR,
  NEURON_TUI_EDITOR: process.env.NEURON_TUI_EDITOR,
  NEURON_EDITOR: process.env.NEURON_EDITOR,

  // Theme preferences
  OPEN_TUI_THEME: process.env.OPEN_TUI_THEME,
  OPENTUI_THEME: process.env.OPENTUI_THEME,

  // Binary build configuration
  NEURON_TUI_IS_BINARY: process.env.NEURON_TUI_IS_BINARY,
  NEURON_TUI_VERSION: process.env.NEURON_TUI_VERSION,
  NEURON_TUI_TARGET: process.env.NEURON_TUI_TARGET,
  NEURON_TUI_RG_PATH: process.env.NEURON_TUI_RG_PATH,
  NEURON_TUI_SCROLL_MULTIPLIER: process.env.NEURON_TUI_SCROLL_MULTIPLIER,
  NEURON_TUI_PERF_TEST: process.env.NEURON_TUI_PERF_TEST,
  NEURON_TUI_LAUNCHER_PID: process.env.NEURON_TUI_LAUNCHER_PID,
  NEURON_TUI_NO_TERMINAL_WATCHDOG: process.env.NEURON_TUI_NO_TERMINAL_WATCHDOG,
  NEURON_TUI_CONFIG_DIR: process.env.NEURON_TUI_CONFIG_DIR,
  NEURON_HOME: process.env.NEURON_HOME,

  TZ: process.env.TZ,
  HTTPS_PROXY: process.env.HTTPS_PROXY,
  https_proxy: process.env.https_proxy,
  HTTP_PROXY: process.env.HTTP_PROXY,
  http_proxy: process.env.http_proxy,
  ALL_PROXY: process.env.ALL_PROXY,
  all_proxy: process.env.all_proxy,
  NODE_TLS_REJECT_UNAUTHORIZED: process.env.NODE_TLS_REJECT_UNAUTHORIZED,
  NODE_EXTRA_CA_CERTS: process.env.NODE_EXTRA_CA_CERTS,
})

/**
 * Get the raw system process.env object.
 * Use this when you need to pass the full environment to subprocesses
 * or when you need to set environment variables at runtime.
 */
export const getSystemProcessEnv = (): NodeJS.ProcessEnv => process.env
