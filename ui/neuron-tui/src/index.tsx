#!/usr/bin/env bun
// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: startup no
// longer loads a Codebuff auth token, handles `login`/`publish`, ships launch
// and crash telemetry, loads `.agents` agents, skills or the tree-sitter wasm
// for the SDK, or starts the engagement and process-census probes. It titles
// the window "Neuron" (saving the console's own title and font for the exit)
// before the watchdog starts, and `--continue` resumes the session saved with
// the chat. See ui/neuron-tui/PORTING.md.

import fs from 'fs'
import os from 'os'

import { getProjectFileTree } from '@neuron-tui/common/project-file-tree'
import {
  createCliRenderer,
  resolveRenderLib,
  setRenderLibPath,
} from '@opentui/core'
import { createRoot } from '@opentui/react'
import {
  QueryClient,
  QueryClientProvider,
  focusManager,
} from '@tanstack/react-query'
import React from 'react'

import { App } from './app'
import { parseArgs } from './cli-args'
import { startEngineSession } from './engine/engine-session'
import { initializeApp } from './init/init-app'
import { getProjectRoot, setProjectRoot } from './project-files'
import { getConfigDir } from './utils/config-dir'
import {
  bunExtractionTempDir,
  defaultRenderLibraryCacheDir,
  ensureRenderLibrary,
  findEmbeddedRenderLibrary,
  sweepLeakedRenderLibraryCopies,
} from './utils/opentui-native-library'
import { trimOversizedChatLogs } from './utils/chat-history'
import { resumeSessionFor } from './utils/run-state-storage'
import { clearLogFile, logger } from './utils/logger'
import { shouldShowProjectPicker } from './utils/project-picker'
import { saveRecentProject } from './utils/recent-projects'
import {
  CLI_RENDERER_EXIT_SIGNALS,
  exitCliWithFatalError,
  installProcessCleanupHandlers,
} from './utils/renderer-cleanup'
import { startTerminalWatchdog } from './utils/terminal-watchdog'
import { restoreConsoleAtExit, startConsoleGuard } from './utils/console-guard'
import { installTerminalProtocolController } from './utils/terminal-protocol-controller'
import { detectTerminalTheme } from './utils/terminal-color-detection'
import { setOscDetectedTheme } from './utils/theme-system'

import type { FileTreeNode } from '@neuron-tui/common/util/file'

// Configure TanStack Query's focusManager for terminal environments
// This is required because there's no browser visibility API in terminal apps
// Without this, refetchInterval won't work because TanStack Query thinks the app is "unfocused"
focusManager.setEventListener(() => {
  // No-op: no event listeners in CLI environment (no window focus/visibility events)
  return () => {}
})
focusManager.setFocused(true)

function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: 5 * 60 * 1000,
        gcTime: 10 * 60 * 1000,
        retry: false,
        refetchOnWindowFocus: false, // CLI doesn't have window focus
        refetchOnReconnect: true,
        refetchOnMount: false,
      },
      mutations: {
        retry: 1,
      },
    },
  })
}

async function main(): Promise<void> {
  // Run OSC theme detection BEFORE anything else.
  // This MUST happen before OpenTUI starts because OSC responses come through stdin,
  // and OpenTUI also listens to stdin. Running detection here ensures stdin is clean.
  if (process.stdin.isTTY && process.platform !== 'win32') {
    try {
      const oscTheme = await detectTerminalTheme()
      if (oscTheme) {
        setOscDetectedTheme(oscTheme)
      }
    } catch {
      // Silently ignore OSC detection failures
    }
  }

  const {
    initialPrompt,
    clearLogs,
    continue: continueChat,
    continueId,
    cwd,
    initialMode,
    engine: engineFlags,
  } = parseArgs()

  // The window is "Neuron" from the start. The console's own title (and, in
  // a classic console window, its font) are saved first, before the
  // watchdog's PowerShell can change them, and given back at exit.
  startConsoleGuard()
  process.once('exit', () => restoreConsoleAtExit())

  await initializeApp({
    cwd,
    continueChat: continueChat ? { chatId: continueId ?? null } : null,
  })

  // Show project picker only when user starts at the home directory or an ancestor
  const projectRoot = getProjectRoot()
  const homeDir = os.homedir()
  const startCwd = process.cwd()
  const showProjectPicker = shouldShowProjectPicker(startCwd, homeDir)

  if (clearLogs) {
    clearLogFile()
  }

  // Reclaim disk from oversized debug logs left by older versions that logged
  // the full conversation to log.jsonl. Deferred to keep the stat sweep over
  // chat directories off the startup path.
  setTimeout(trimOversizedChatLogs, 0)

  const queryClient = createQueryClient()

  // The engine runs in the project folder, so it starts once one is chosen.
  // `--continue` resumes the engine session the chat was saved with (saved
  // as soon as the engine reported it, so also after an interrupted turn).
  const resumeSession = continueChat
    ? resumeSessionFor(continueId ?? undefined)
    : undefined
  if (!showProjectPicker) {
    void startEngineSession({
      ...engineFlags,
      cwd: projectRoot,
      resume: resumeSession,
    })
  }

  const AppRoot = () => {
    const [fileTree, setFileTree] = React.useState<FileTreeNode[]>([])
    const [currentProjectRoot, setCurrentProjectRoot] =
      React.useState(projectRoot)
    const [showProjectPickerScreen, setShowProjectPickerScreen] =
      React.useState(showProjectPicker)

    const loadFileTree = React.useCallback(async (root: string) => {
      try {
        if (root) {
          const tree = await getProjectFileTree({
            projectRoot: root,
            fs: fs.promises,
          })
          setFileTree(tree)
        }
      } catch (error) {
        // Silently fail - fileTree is optional for @ menu
      }
    }, [])

    React.useEffect(() => {
      loadFileTree(currentProjectRoot)
    }, [currentProjectRoot, loadFileTree])

    // Callback for when user selects a new project from the picker
    const handleProjectChange = React.useCallback(
      async (newProjectPath: string) => {
        // Change process working directory
        process.chdir(newProjectPath)
        // Update the project root in the module state
        setProjectRoot(newProjectPath)
        // Save to recent projects list
        saveRecentProject(newProjectPath)
        // The engine works in one folder: restart it in the new one.
        void startEngineSession({ ...engineFlags, cwd: newProjectPath })
        // Update local state
        setCurrentProjectRoot(newProjectPath)
        // Reset file tree state to trigger reload
        setFileTree([])
        // Hide the picker and show the chat
        setShowProjectPickerScreen(false)
      },
      [],
    )

    return (
      <App
        initialPrompt={initialPrompt}
        fileTree={fileTree}
        continueChat={continueChat}
        continueChatId={continueId ?? undefined}
        initialMode={initialMode}
        showProjectPicker={showProjectPickerScreen}
        onProjectChange={handleProjectChange}
      />
    )
  }

  // Install early error handlers BEFORE renderer creation.
  // If the renderer crashes during init, these ensure the error is visible
  // by exiting the alternate screen buffer before printing the error.
  const earlyFatalHandler = (error: unknown) =>
    exitCliWithFatalError('Fatal error during startup', error)
  process.on('uncaughtException', earlyFatalHandler)
  process.on('unhandledRejection', earlyFatalHandler)

  // Last line of defense for uncatchable deaths (SIGKILL, native crashes,
  // kill sweeps that also take out the launcher): a detached process
  // (sh on POSIX, PowerShell on Windows) that resets the terminal when this
  // process disappears. Started before the renderer begins enabling terminal
  // modes; the clean-shutdown path (renderer-cleanup) disarms it.
  startTerminalWatchdog()

  // Load the native renderer up front, so a temp directory that is full,
  // read-only or noexec gets a fallback copy and, failing that, an actionable
  // message instead of "Failed to initialize OpenTUI render library".
  const renderLibrarySource = await ensureRenderLibrary({
    resolve: resolveRenderLib,
    setPath: setRenderLibPath,
    embeddedFiles: () => Bun.embeddedFiles,
    cacheDir: () => defaultRenderLibraryCacheDir(getConfigDir()),
    tempDir: () => bunExtractionTempDir(),
  })
  if (renderLibrarySource === 'cache') {
    logger.warn(
      { tempDir: bunExtractionTempDir() },
      'Loaded the terminal renderer from the config directory: the temp directory copy failed',
    )
  }
  // Bun leaves a full copy of the renderer library in the temp directory on
  // every launch; reclaim older ones off the startup path.
  setTimeout(() => {
    const library = findEmbeddedRenderLibrary(Bun.embeddedFiles)
    if (!library) return
    void sweepLeakedRenderLibraryCopies({
      library,
      tempDir: bunExtractionTempDir(),
    })
      .then(({ removed, bytes }) => {
        if (removed > 0) {
          logger.info(
            { removed, bytes },
            'Removed stale renderer library copies from the temp directory',
          )
        }
      })
      .catch(() => {})
  }, 0)
  const renderer = await createCliRenderer({
    backgroundColor: 'transparent',
    exitOnCtrlC: false,
    exitSignals: CLI_RENDERER_EXIT_SIGNALS,
    screenMode: 'alternate-screen',
  })

  // Install the renderer-aware handlers before removing the startup safety net
  // so an installation failure still restores the terminal and reports itself.
  installProcessCleanupHandlers(renderer)
  const terminalProtocols = installTerminalProtocolController(renderer, {
    onError: (error) =>
      logger.debug(error, 'Terminal protocol transition failed'),
  })
  renderer.once('destroy', () => terminalProtocols.dispose())
  process.removeListener('uncaughtException', earlyFatalHandler)
  process.removeListener('unhandledRejection', earlyFatalHandler)

  createRoot(renderer).render(
    <QueryClientProvider client={queryClient}>
      <AppRoot />
    </QueryClientProvider>,
  )
}

void main()
