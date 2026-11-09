import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import {
  FEATURE_ACCOUNT,
  FEATURE_EXTENSIONS,
  FEATURE_MCP_TOGGLE,
  FEATURE_SESSION_SCOPE,
  FEATURE_SESSIONS,
  FEATURE_TRUST_PREVIEW,
  FEATURE_TRUST_WORKSPACE,
  changedPaths,
  hasFeature,
  needsSignIn,
  openQuestionItem,
  runningSubagents,
  sessionAccount,
  type SessionController,
  type TranscriptItem,
} from '@neuron/client'

import { AccountRow, SignOutDialog, type AccountAction } from './components/AccountMenu'
import { ChatHeader, RestartBanner, StatusBar } from './components/Bars'
import { ChatContextBar, DiscardDialog } from './components/ChatHeader'
import { CommandPalette, ShortcutsSheet } from './components/CommandPalette'
import { Composer, EffortPicker, ModeMenu, nextMode, type ComposerHandle } from './components/Composer'
import { ExtensionsPanel } from './components/ExtensionsPanel'
import { FirstRun } from './components/FirstRun'
import { ModelPicker } from './components/ModelPicker'
import { AllSessionsPage, ExtensionsPage, ProjectsPage, UpdateNotice } from './components/Pages'
import { PermissionDialog } from './components/PermissionDialog'
import { PixelLogo } from './components/PixelLogo'
import { EngineProblemCard } from './components/ProblemCard'
import { SettingsPage, type SettingsSection, type UpdateCheck } from './components/SettingsPage'
import { SidePane, type PaneTab } from './components/SidePane'
import { Sidebar, type NavTarget } from './components/Sidebar'
import { SignInPanel } from './components/SignIn'
import { TrustModal } from './components/TrustModal'
import { Transcript } from './components/Transcript'
import { TodoStrip } from './components/TodoStrip' // L1: live transcript
import { useChats } from './chats'
import { NOT_KEEPING_UP } from './platform/chat-host'
import { buildDiagnostics, engineProblemFor, parseEngineProblem } from './engineProblems'
import { exportName, transcriptJson, transcriptMarkdown, type DesktopCommand } from './commands'
import { useSession } from './hooks'
import { LINKS } from './links'
import { ChatError, type EngineInfo, type LaunchOptions, type Platform, type Project, type UpdateInfo } from './platform'
import { safeExternalUrl } from './platform/links'
import type { PaletteCommand } from './palette'
import { noteSeen, sessionRows, type SessionRow } from './sessions'
import { useSeen, useSettings, type Settings } from './settings'
import { shortcutFor, shortcutKeys, type ShortcutId } from './shortcuts'

type ToolItem = Extract<TranscriptItem, { kind: 'tool' }>
type View = 'chat' | NavTarget | 'settings'

const STARTERS = [
  'Explain how this project is organised',
  'Find likely bugs in the code I changed recently',
  'Write tests for the least-tested module',
]

/** The sidebar becomes an icon rail below this width (unless set otherwise). */
const NARROW = '(max-width: 1100px)'

function useNarrow(): boolean {
  const [narrow, setNarrow] = useState(() => window.matchMedia?.(NARROW).matches ?? false)
  useEffect(() => {
    const media = window.matchMedia?.(NARROW)
    if (!media) return
    const apply = () => setNarrow(media.matches)
    media.addEventListener?.('change', apply)
    return () => media.removeEventListener?.('change', apply)
  }, [])
  return narrow
}

/** What a new engine in `folder` starts with. */
function launchOptions(settings: Settings, folder: string | null, reasoningEffort?: LaunchOptions['reasoningEffort']): LaunchOptions {
  const remembered = settings.rememberModePerFolder && folder ? settings.modeByFolder[folder] : undefined
  return {
    model: settings.defaultModel.trim() || undefined,
    permissionMode: remembered ?? settings.defaultPermissionMode,
    reasoningEffort, // L3: composer
  }
}

export function App({ platform }: { platform: Platform }) {
  const [settings, updateSettings, settingsLoaded] = useSettings(platform.store)
  // L4: a message for the user (a full house of chats, notifications off).
  const [toast, setToast] = useState<string | null>(null)
  // A restart that failed; the engines' own start errors come from `useChats`.
  const [restartError, setLaunchError] = useState<string | null>(null)
  const [palette, setPalette] = useState(false)
  const [shortcutsOpen, setShortcutsOpen] = useState(false)
  // The chat whose copy the discard dialog is about, and whether the person got there by closing it.
  const [discarding, setDiscarding] = useState<string | null>(null)
  const [discardFromClose, setDiscardFromClose] = useState(false)
  // Why a chat could not replace the one just closed (the last one).
  const [replaceError, setReplaceError] = useState<string | null>(null)
  const switchingRef = useRef(false)
  const paletteOpen = useRef(false)
  paletteOpen.current = palette
  const lastPath = useRef<string | undefined>(undefined)
  const cycleModeRef = useRef<() => void>(() => {})
  // The mode a cycling press leaves behind, until the engine's own status says so.
  const cycled = useRef<{ from: unknown; mode: string; plan: boolean } | null>(null)
  // L3: composer. The reasoning effort new engines start with, and a change waiting for the turn to end.
  const [effort, setEffort] = useState<LaunchOptions['reasoningEffort']>(undefined)
  const [effortPending, setEffortPending] = useState(false)
  const effortRef = useRef<LaunchOptions['reasoningEffort']>(undefined)
  // The engines start once the settings say how.
  useEffect(() => {
    if (settingsLoaded) platform.setLaunchOptions(launchOptions(settings, null, effortRef.current))
    // Settings changed later apply to the next engine, not this one.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [platform, settingsLoaded])
  const { store: chats, snapshot, active, launchError, retry: retryFirstChat } = useChats(platform, settingsLoaded, setToast)
  const multi = platform.can.multiChat && chats !== null
  const controller: SessionController | null = active?.controller ?? null
  const state = useSession(controller)
  const [seen, updateSeen] = useSeen(platform.store)
  const [view, setView] = useState<View>('chat')
  const [section, setSection] = useState<SettingsSection>('account')
  const [paneOpen, setPaneOpen] = useState(true)
  const [paneTab, setPaneTab] = useState<PaneTab>('changes')
  const [terminalOpened, setTerminalOpened] = useState(false)
  const [toolKey, setToolKey] = useState<string | null>(null)
  const [recent, setRecent] = useState<Project[]>([])
  const [projectError, setProjectError] = useState<string | null>(null)
  const [switching, setSwitching] = useState(false)
  const [railOverride, setRailOverride] = useState<boolean | null>(null)
  const [signingOut, setSigningOut] = useState<'confirm' | 'restart' | null>(null)
  const [trusting, setTrusting] = useState(false)
  const [appVersion, setAppVersion] = useState<string | null>(null)
  const [update, setUpdate] = useState<UpdateInfo | null>(null)
  const [updateDismissed, setUpdateDismissed] = useState(false)
  const [updateCheck, setUpdateCheck] = useState<UpdateCheck>({ state: 'idle' })
  const [engineInfo, setEngineInfo] = useState<EngineInfo | null>(null) // L5
  const search = useRef<HTMLInputElement>(null)
  const composer = useRef<ComposerHandle>(null)
  const [modelSignal, setModelSignal] = useState(0)
  const narrow = useNarrow()

  // The version, and whether a newer release exists, once per launch.
  useEffect(() => {
    void platform.appVersion().then(setAppVersion, () => {})
    if (platform.can.updates && !platform.manualUpdates) void platform.checkForUpdate().then(setUpdate, () => {})
  }, [platform])

  // L5: which engine runs (bundled, NEURON_BIN or PATH).
  useEffect(() => {
    void platform.engineInfo().then(setEngineInfo, () => {})
  }, [platform])

  const refreshRecent = useCallback(() => {
    void platform.recentProjects().then(setRecent, () => setRecent([]))
  }, [platform])
  useEffect(() => refreshRecent(), [refreshRecent])

  // With several chats, one that is working does not stop the others being opened.
  const uiBusy = multi ? switching : state.busy || switching
  const history = hasFeature(state, FEATURE_SESSIONS)
  const ready = state.connection === 'ready'
  const sessionId = state.status?.session_id || null
  const project = multi ? (active?.info.project ?? null) : platform.project()
  const folder = state.status?.cwd ?? project?.path ?? null
  if (project?.path) lastPath.current = project.path

  // The session list follows the engine: after it starts, after every turn, and on a switch.
  // Engines that can list every project's sessions do.
  const allProjects = hasFeature(state, FEATURE_SESSION_SCOPE)
  useEffect(() => {
    if (controller && ready && history && !state.busy) controller.listSessions(allProjects ? 'all' : undefined)
  }, [controller, ready, history, allProjects, state.busy, sessionId])

  // Sessions seen for the first time are not unread; the open one is always seen.
  useEffect(() => {
    if (state.sessions) updateSeen((current) => noteSeen(current, state.sessions!, sessionId))
  }, [state.sessions, sessionId, updateSeen])

  // A turn that ended in an error marks its session failed; a good one clears it.
  // Per chat: switching from a working chat to an idle one is not a turn ending.
  const wasBusy = useRef(new Map<string, boolean>())
  const chatKey = active?.id ?? ''
  useEffect(() => {
    if (wasBusy.current.get(chatKey) && !state.busy && sessionId) {
      const lastUser = state.items.map((item) => item.kind).lastIndexOf('user')
      const failed = state.items.slice(lastUser + 1).some((item) => item.kind === 'error')
      updateSeen((current) => {
        if (Boolean(current.failed[sessionId]) === failed) return current
        const next = { ...current.failed }
        if (failed) next[sessionId] = true
        else delete next[sessionId]
        return { ...current, failed: next }
      })
    }
    wasBusy.current.set(chatKey, state.busy)
  }, [state.busy, state.items, sessionId, updateSeen, chatKey])

  // A folder remembered in another mode starts in it.
  const modeApplied = useRef<string | null>(null)
  useEffect(() => {
    const status = state.status
    if (!controller || !ready || !status?.cwd || modeApplied.current === status.cwd || state.busy) return
    modeApplied.current = status.cwd
    const remembered = settings.rememberModePerFolder ? settings.modeByFolder[status.cwd] : undefined
    if (remembered && remembered !== status.permission_mode) controller.command('permissions', remembered)
  }, [controller, ready, state.status, state.busy, settings.rememberModePerFolder, settings.modeByFolder])

  const rows = useMemo<SessionRow[] | null>(
    () => (state.sessions ? sessionRows(state.sessions, { open: project, currentId: sessionId, busy: state.busy, seen }) : null),
    [state.sessions, project, sessionId, state.busy, seen],
  )

  // Open chats are listed above the history, not in it.
  const historyRows = useMemo(() => {
    if (!rows) return rows
    const open = new Set(snapshot.items.map((item) => item.sessionId).filter(Boolean))
    return rows.filter((row) => !open.has(row.id))
  }, [rows, snapshot.items])
  // L2 + L4: the chat bar's "Open in editor" opens the chat's own folder (its copy of the project).
  const openInEditor = useCallback(() => {
    void platform.openInEditor('.').catch((failure: Error) => setToast(failure.message))
  }, [platform])

  const openPane = useCallback((tab: PaneTab) => {
    if (tab === 'terminal') setTerminalOpened(true)
    setPaneTab(tab)
    setPaneOpen(true)
  }, [])

  const refreshExtensions = useCallback(() => {
    if (ready) controller?.listExtensions()
  }, [controller, ready])

  // L4: a chat in its own folder or copy; a refusal (the 4 working chats) is told, not thrown.
  const openChat = useCallback(
    async (request: { path?: string; resume?: string | null; worktree?: { name?: string } }) => {
      if (!chats || switchingRef.current) return
      switchingRef.current = true
      setSwitching(true)
      try {
        await chats.open(request)
        setToolKey(null)
        setView('chat')
      } catch (failure) {
        setToast(failure instanceof Error ? failure.message : String(failure))
        throw failure
      } finally {
        switchingRef.current = false
        setSwitching(false)
      }
    },
    [chats],
  )

  const newSession = useCallback(() => {
    if (multi) {
      void openChat({ path: project?.path }).catch(() => {})
      return
    }
    if (!controller || state.busy) return
    setToolKey(null)
    setView('chat')
    controller.newChat()
  }, [multi, openChat, project, controller, state.busy])

  const newWorktreeChat = useCallback(() => {
    if (multi) void openChat({ path: project?.path, worktree: {} }).catch(() => {})
  }, [multi, openChat, project])

  const openFolder = useCallback(
    async (path: string, resume: string | null = null) => {
      if (switchingRef.current) return
      switchingRef.current = true
      setSwitching(true)
      setProjectError(null)
      try {
        platform.setLaunchOptions(launchOptions(settings, path, effortRef.current))
        modeApplied.current = path
        if (multi && chats) {
          // A chat already open is shown, not opened twice.
          const open = resume ? chats.findSession(resume) : chats.findProject(path)
          if (open) chats.setActive(open.id)
          else await chats.open({ path, resume })
        } else {
          await platform.openProject(path, resume)
        }
        setToolKey(null)
        setView('chat')
        refreshRecent()
      } catch (failure) {
        const message = failure instanceof Error ? failure.message : String(failure)
        setProjectError(message)
        if (multi) setToast(message)
        throw failure
      } finally {
        switchingRef.current = false
        setSwitching(false)
      }
    },
    [platform, settings, refreshRecent, multi, chats],
  )

  const pickFolder = useCallback(async () => {
    const picked = await platform.pickFolder()
    if (picked) await openFolder(picked).catch(() => {})
  }, [platform, openFolder])

  const openSession = useCallback(
    (row: SessionRow) => {
      if (!controller) return
      setToolKey(null)
      setView('chat')
      if (multi) void openFolder(row.project.path || project?.path || '', row.id).catch(() => {})
      else if (!row.project.path || row.project.path === project?.path) controller.resume(row.id)
      else void openFolder(row.project.path, row.id).catch(() => {})
    },
    [controller, project, openFolder, multi],
  )

  const rail = railOverride ?? (settings.sidebar === 'rail' || (settings.sidebar === 'auto' && narrow))
  const focusSearch = useCallback(() => {
    setRailOverride(false)
    requestAnimationFrame(() => search.current?.focus())
  }, [])
  const openSettings = useCallback((at: SettingsSection = section) => {
    setSection(at)
    setView('settings')
  }, [section])

  // L4: the shortcuts of `shortcuts.ts`, which the shortcuts sheet lists too.
  const runShortcut = useCallback(
    (id: ShortcutId) => {
      if (id === 'palette') setPalette(true)
      else if (id === 'newChat') newSession()
      else if (id === 'newWorktreeChat') newWorktreeChat()
      else if (id === 'nextWaiting') chats?.jumpToWaiting()
      else if (id === 'previousChat') chats?.step(-1)
      else if (id === 'nextChat') chats?.step(1)
      else if (id === 'cycleMode') cycleModeRef.current()
      else setShortcutsOpen(true)
    },
    [newSession, newWorktreeChat, chats],
  )

  // The handler reads the latest actions, so a key pressed as the chats appear is not lost.
  const shortcutAction = useRef(runShortcut)
  shortcutAction.current = runShortcut

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      // A held key is one press.
      if (event.repeat) return
      if (event.key === 'Escape' && view !== 'chat' && !document.querySelector('[aria-modal="true"], [role="menu"]')) {
        setView('chat')
        return
      }
      if (!(event.ctrlKey || event.metaKey)) return
      const shortcut = shortcutFor(event)
      if (shortcut) {
        // The composer already cycled the mode for its own key press.
        const handled = event.defaultPrevented
        event.preventDefault()
        // The palette's own key closes it; any other dialog keeps the keys to itself.
        if (shortcut === 'palette' && paletteOpen.current) setPalette(false)
        else if (document.querySelector('[aria-modal="true"]')) return
        else if (shortcut === 'cycleMode' && handled) return
        else shortcutAction.current(shortcut)
        return
      }
      const key = event.key.toLowerCase()
      if (event.key === '`') {
        event.preventDefault()
        if (paneOpen && paneTab === 'terminal') setPaneOpen(false)
        else openPane('terminal')
      } else if (key === 'b' && event.shiftKey) {
        event.preventDefault()
        setRailOverride(!rail)
      } else if (key === 'b') {
        event.preventDefault()
        setPaneOpen((open) => !open)
      } else if (event.key === ',') {
        event.preventDefault()
        openSettings()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [paneOpen, paneTab, openPane, openSettings, rail, view])

  // L4: what the palette lists. Other lanes append their own commands here.
  const replaceChat = useCallback(async () => {
    setReplaceError(null)
    try {
      await openChat({ path: lastPath.current })
    } catch (failure) {
      setReplaceError(failure instanceof Error ? failure.message : String(failure))
    }
  }, [openChat])
  const closeChat = useCallback(
    async (id: string) => {
      if (!chats) return
      try {
        await chats.close(id)
      } catch (failure) {
        const info = chats.getSnapshot().items.find((item) => item.id === id)?.info
        // A chat with a copy of its own is closed by deciding what happens to the copy.
        if (failure instanceof ChatError && failure.code === 'has_worktree' && info?.worktree) {
          setDiscardFromClose(true)
          setDiscarding(id)
        } else setToast(failure instanceof Error ? failure.message : String(failure))
        return
      }
      // A window always has a chat: closing the last one opens a fresh one.
      if (chats.getSnapshot().items.length === 0) await replaceChat()
    },
    [chats, replaceChat],
  )
  const paletteCommands = useMemo<PaletteCommand[]>(() => {
    const actions: PaletteCommand[] = [
      { id: 'new-chat', title: 'New chat', group: 'Actions', keys: shortcutKeys('newChat'), run: newSession },
      ...(multi ? [{ id: 'new-copy', title: 'New chat in its own copy of the project', group: 'Actions', keys: shortcutKeys('newWorktreeChat'), run: newWorktreeChat }] : []),
      ...(multi && snapshot.needsYouCount > 0
        ? [{ id: 'jump', title: 'Go to the chat that needs you', group: 'Actions', keys: shortcutKeys('nextWaiting'), run: () => void chats?.jumpToWaiting() }]
        : []),
      ...(multi && snapshot.items.length > 0 && active
        ? [{ id: 'close-chat', title: 'Close this chat', group: 'Actions', run: () => void closeChat(active.id) }]
        : []),
      { id: 'search', title: 'Search sessions', group: 'Actions', run: focusSearch },
      { id: 'projects', title: 'Projects', group: 'Actions', run: () => setView('projects') },
      { id: 'extensions', title: 'Extensions', group: 'Actions', run: () => setView('extensions') },
      { id: 'settings', title: 'Settings', group: 'Actions', keys: 'Ctrl+,', run: () => openSettings() },
      { id: 'shortcuts', title: 'Keyboard shortcuts', group: 'Actions', keys: shortcutKeys('shortcuts'), run: () => setShortcutsOpen(true) },
    ]
    const openChats: PaletteCommand[] = multi
      ? snapshot.items.map((item) => ({ id: `chat-${item.id}`, title: `${item.title} (${item.projectName})`, group: 'Chats', run: () => chats?.setActive(item.id) }))
      : []
    const sessions: PaletteCommand[] = (rows ?? []).slice(0, 40).map((row) => ({ id: `session-${row.id}`, title: row.title, group: 'Sessions', run: () => openSession(row) }))
    return [...actions, ...openChats, ...sessions]
  }, [multi, snapshot, chats, active, rows, newSession, newWorktreeChat, closeChat, focusSearch, openSettings, openSession])

  // A chat switch starts on the chat's own page, with nothing selected.
  useEffect(() => {
    setToolKey(null)
    setView('chat')
  }, [active?.id])

  // A message stays a few seconds.
  useEffect(() => {
    if (!toast) return
    const timer = setTimeout(() => setToast(null), 8000)
    return () => clearTimeout(timer)
  }, [toast])

  // Signing out: the engine forgets the sign-in, then a fresh engine shows the sign-in screen.
  useEffect(() => {
    if (signingOut === 'restart' && !state.busy) {
      setSigningOut(null)
      void platform.restart(sessionId).catch((error: Error) => setLaunchError(error.message))
    }
  }, [signingOut, state.busy, platform, sessionId])

  // Once allowed, the folder's own settings load in a fresh engine, on the same session.
  // The home folder is trusted for that engine only, so it is told so.
  const trustedOnce = useRef(false)
  useEffect(() => {
    if (!trusting || !state.restartRequired) return
    setTrusting(false)
    if (platform.can.restart) {
      const keep = state.items.some((item) => item.kind === 'user') ? sessionId : null
      void platform.restart(keep, { trustOnce: trustedOnce.current }).catch((error: Error) => setLaunchError(error.message))
    }
  }, [trusting, state.restartRequired, state.items, platform, sessionId])

  const changes = useMemo(() => changedPaths(state), [state])
  // Re-read the tree, git and open files whenever a call finishes.
  const refreshKey = useMemo(
    () => state.items.filter((item) => item.kind === 'tool' && item.state !== 'writing' && item.state !== 'running' && item.state !== 'awaiting_permission').length,
    [state.items],
  )
  const tool = (state.items.find((item) => item.key === toolKey) as ToolItem | undefined) ?? null
  const permission = state.permissions[0] ?? null
  const signIn = needsSignIn(state) || state.signInCode !== null || state.sessionStarting
  const question = openQuestionItem(state)
  const account = sessionAccount(state)
  const untrusted =
    hasFeature(state, FEATURE_TRUST_WORKSPACE) && state.status !== null && !state.status.workspace_trusted && !state.restartRequired
  // The discard dialog is about the chat it was opened for, whichever chat is in view later.
  const discardItem = discarding ? snapshot.items.find((item) => item.id === discarding) : null
  // A message the engine's queue refused comes back to the composer.
  const refusedPrompt = useMemo(() => {
    const [prev, last] = state.items.slice(-2)
    return last?.kind === 'error' && last.text === NOT_KEEPING_UP && prev?.kind === 'user' ? { text: prev.text, attachments: prev.attachments } : null
  }, [state.items])
  const firstRun = settingsLoaded && !settings.firstRunDone && controller !== null

  // L3: a new reasoning effort applies in a fresh engine on this chat, once the turn is over.
  // It reads the controller as it is when the effect fires, not this render's snapshot, and never
  // restarts under an open question or approval. The queue holds (`effortPending`) until the restart is over.
  const restartingEffort = useRef(false)
  useEffect(() => {
    if (!effortPending || restartingEffort.current || !controller) return
    const latest = controller.getState()
    if (latest.connection !== 'ready' || latest.busy || latest.permissions.length > 0 || openQuestionItem(latest) !== null) return
    restartingEffort.current = true
    const keep = latest.items.some((item) => item.kind === 'user') ? latest.status?.session_id || null : null
    void platform
      .restart(keep)
      .catch((error: Error) => setLaunchError(error.message))
      .finally(() => {
        restartingEffort.current = false
        setEffortPending(false)
      })
  }, [effortPending, ready, state.busy, permission, question, state.items, controller, platform])

  // What allowing the folder would turn on, asked for once per folder.
  const canPreview = hasFeature(state, FEATURE_TRUST_PREVIEW)
  const previewAsked = useRef<string | null>(null)
  useEffect(() => {
    if (!controller || !ready || !untrusted || !folder || !canPreview || previewAsked.current === folder) return
    previewAsked.current = folder
    controller.trustPreview()
  }, [controller, ready, untrusted, folder, canPreview])

  // The usage page reads the plan and usage again.
  const canRefreshAccount = hasFeature(state, FEATURE_ACCOUNT)
  useEffect(() => {
    if (view === 'settings' && section === 'usage' && controller && ready && canRefreshAccount) controller.refreshAccount()
  }, [view, section, controller, ready, canRefreshAccount])
  const title =
    rows?.find((row) => row.id === sessionId)?.title ??
    (state.items.find((item) => item.kind === 'user') as Extract<TranscriptItem, { kind: 'user' }> | undefined)?.text.split('\n')[0] ??
    'New session'

  const openLink = (url: string) => {
    const safe = safeExternalUrl(url)
    if (safe) platform.openExternal(safe)
  }
  const signInNow = () => {
    setView('chat')
    controller?.slash('login')
  }
  const accountAction = (action: AccountAction) => {
    if (action === 'settings') openSettings('account')
    else if (action === 'usage') openSettings('usage')
    else if (action === 'whats-new') openSettings('about')
    else setSigningOut('confirm')
  }
  const checkUpdates = () => {
    if (platform.manualUpdates) return setUpdateCheck({ state: 'manual' })
    setUpdateCheck({ state: 'checking' })
    void platform.checkForUpdate().then(
      (found) => {
        setUpdate(found)
        setUpdateCheck(found ? { state: 'available', update: found } : { state: 'latest' })
      },
      () => setUpdateCheck({ state: 'latest' }),
    )
  }

  // A start that failed: the first chat's engine (L4), or a restart before a chat was in view.
  const startError = launchError ?? restartError
  // L5: the support report, and restarting the engine with a card when that fails.
  const copyDiagnostics = async () => {
    const hostReport = await platform.hostDiagnostics().catch(() => null)
    const lastItem = [...state.items].reverse().find((item) => item.kind === 'error')
    const text = buildDiagnostics({
      appVersion,
      hostReport,
      engine: { version: state.version, protocol: state.protocol, features: state.features },
      connection: state.connection,
      lastError: state.connectionError ?? startError ?? (lastItem && 'text' in lastItem ? lastItem.text : null),
      updates: platform.manualUpdates ? 'manual' : platform.can.updates ? 'signed' : 'none',
    })
    await navigator.clipboard.writeText(text)
  }
  const restartEngine = () => {
    void platform.restart(sessionId).catch((error: Error) => {
      const problem = engineProblemFor(error.message, { restart: true })
      controller?.apply({ type: 'local/disconnected', error: problem ? `[engine:${problem.code}] ${problem.message}` : error.message })
    })
  }
  const exitedProblem = state.connection === 'exited' ? parseEngineProblem(state.connectionError) : null

  if (!controller) {
    const shownError = startError ?? replaceError
    const problem = shownError ? parseEngineProblem(shownError) : null
    // Starts again: the first chat of the window, or the one that replaces a chat just closed.
    const retryStart = () => {
      setLaunchError(null)
      if (chats) void replaceChat()
      else retryFirstChat()
    }
    return (
      <div className="splash">
        <PixelLogo pixel={10} />
        {shownError && !switching ? (
          <>
            {problem ? (
              <EngineProblemCard problem={problem} actions={{ onCopyDiagnostics: copyDiagnostics, onOpenLink: openLink }} />
            ) : (
              <div className="error-line" role="alert">
                {shownError}
              </div>
            )}
            <button className="primary" onClick={retryStart}>
              Retry
            </button>
          </>
        ) : (
          <div className="muted" role="status">
            Starting the Neuron engine…
          </div>
        )}
      </div>
    )
  }

  // L3: composer. The permission mode as the mode menu applies it, and Ctrl+Shift+M (Cmd+Shift+M) through them.
  const applyMode = (mode: string) => {
    controller.command('permissions', mode)
    if (settings.rememberModePerFolder && folder) updateSettings({ modeByFolder: { ...settings.modeByFolder, [folder]: mode } })
  }
  const applyPlan = (on: boolean) => controller.command('plan', on ? 'on' : 'off')
  // The mode a press leaves behind, until the engine's own status says so: two quick presses advance twice.
  const cycleMode = () => {
    const latest = controller.getState()
    if (latest.connection !== 'ready' || latest.busy) return
    const pending = cycled.current?.from === latest.status ? cycled.current : null
    const mode = pending?.mode ?? latest.status?.permission_mode ?? 'workspace-write'
    const plan = pending?.plan ?? latest.status?.plan_mode ?? false
    const next = nextMode(mode, plan)
    cycled.current = { from: latest.status, mode: next.mode, plan: next.plan }
    if (next.plan) return applyPlan(true)
    if (plan) applyPlan(false)
    if (next.mode !== mode) applyMode(next.mode)
  }
  cycleModeRef.current = cycleMode
  const changeEffort = (value: LaunchOptions['reasoningEffort']) => {
    effortRef.current = value
    setEffort(value)
    platform.setLaunchOptions(launchOptions(settings, folder, value))
    if (platform.can.restart) setEffortPending(true)
  }
  const runCommand = async (name: DesktopCommand, args: string): Promise<string | void> => {
    switch (name) {
      case 'new':
        return newSession()
      case 'plan':
        return applyPlan(!(state.status?.plan_mode ?? false))
      case 'history':
        return setView('sessions')
      case 'model':
        if (args) controller.setModel(args)
        else setModelSignal((count) => count + 1)
        return
      case 'copy':
        try {
          await navigator.clipboard.writeText(transcriptMarkdown(state.items, title))
          return 'Copied the chat as Markdown.'
        } catch {
          return 'Could not copy the chat.'
        }
      case 'export': {
        const format = args.toLowerCase().startsWith('json') ? 'json' : 'md'
        const content = format === 'json' ? transcriptJson(state.items) : transcriptMarkdown(state.items, title)
        const saved = await platform.exportSave(exportName(title, format), content)
        return saved ? `Saved to ${saved}` : 'Not saved.'
      }
      case 'help':
        return
    }
  }

  const signInPanel = (
    <SignInPanel
      code={state.signInCode}
      starting={state.sessionStarting}
      busy={state.busy}
      problem={state.problem && (state.problem.kind === 'unreachable' || state.problem.kind === 'busy') ? state.problem : null}
      onSignIn={() => controller.slash('login')}
      onCancel={() => controller.interrupt()}
      onOpen={openLink}
    />
  )

  const footer = (
    <>
      {update && !updateDismissed && (
        <UpdateNotice update={update} rail={rail} onDownload={() => openLink(LINKS.download)} onDismiss={() => setUpdateDismissed(true)} />
      )}
      <AccountRow account={account} rail={rail} onSignIn={signInNow} onAction={accountAction} onOpenLink={openLink} onSearch={focusSearch} />
    </>
  )

  return (
    <div className={`app${paneOpen && view === 'chat' ? '' : ' pane-closed'}${rail ? ' rail' : ''}`}>
      <Sidebar
        ref={search}
        rail={rail}
        onToggleRail={() => setRailOverride(!rail)}
        rows={history ? (multi ? historyRows : rows) : null}
        open={project}
        groupBy={settings.groupBy}
        onGroupBy={(groupBy) => updateSettings({ groupBy })}
        busy={uiBusy}
        historyAvailable={history}
        page={view}
        onNewSession={newSession}
        onOpenSession={openSession}
        onOpenProject={(target) => void openFolder(target.path).catch(() => {})}
        onNavigate={(target) => {
          if (target === 'projects') refreshRecent()
          setView(target)
        }}
        onSettings={() => openSettings()}
        onSearch={focusSearch}
        footer={footer}
        // L4: multi-chat
        chats={multi ? snapshot.items : undefined}
        activeChat={snapshot.activeId}
        onOpenChat={(id) => chats?.setActive(id)}
        needsYouCount={multi ? snapshot.needsYouCount : 0}
        onJumpWaiting={() => chats?.jumpToWaiting()}
        onNewWorktreeChat={multi ? newWorktreeChat : undefined}
        onCloseChat={multi ? (id) => void closeChat(id) : undefined}
      />
      <main className="chat" aria-label={view === 'chat' ? 'Session' : undefined}>
        {view === 'settings' && (
          <SettingsPage
            section={section}
            onSection={setSection}
            settings={settings}
            onChange={updateSettings}
            account={account}
            models={state.models}
            appVersion={appVersion}
            engineVersion={state.version}
            where={platform.label}
            canUpdate={platform.can.updates}
            update={updateCheck}
            onCheckUpdates={checkUpdates}
            onOpenLink={openLink}
            onSignIn={signInNow}
            onSignOut={() => setSigningOut('confirm')}
            onClose={() => setView('chat')}
            engineInfo={engineInfo} // L5
            engineDetails={{ protocol: state.protocol, features: state.features }}
            onCopyDiagnostics={copyDiagnostics}
          />
        )}
        {view === 'projects' && (
          <ProjectsPage
            current={project}
            recent={recent}
            counts={(rows ?? []).reduce<Record<string, number>>((counts, row) => ({ ...counts, [row.project.path]: (counts[row.project.path] ?? 0) + 1 }), {})}
            canPickFolder={platform.can.pickFolder}
            canOpen={platform.can.openProject}
            busy={uiBusy}
            error={projectError}
            onPick={() => void pickFolder()}
            onOpen={(path) => void openFolder(path).catch(() => {})}
            onClose={() => setView('chat')}
          />
        )}
        {view === 'sessions' && (
          <AllSessionsPage rows={rows ?? []} groupBy={settings.groupBy} busy={uiBusy} onOpen={openSession} onClose={() => setView('chat')} />
        )}
        {view === 'extensions' && (
          <ExtensionsPage onClose={() => setView('chat')}>
            {hasFeature(state, FEATURE_EXTENSIONS) ? (
              <ExtensionsPanel
                extensions={state.extensions}
                onRefresh={refreshExtensions}
                onSetEnabled={hasFeature(state, FEATURE_MCP_TOGGLE) ? (name, enabled) => controller.setMcpServerEnabled(name, enabled) : undefined}
              />
            ) : (
              <p className="muted">This engine does not report its extensions; update Neuron for this page.</p>
            )}
          </ExtensionsPage>
        )}
        {view === 'chat' && (
          <>
            <ChatHeader
              state={state}
              title={title}
              project={project?.name ?? null}
              where={platform.label}
              sidePaneOpen={paneOpen}
              onToggleSidePane={() => setPaneOpen(!paneOpen)}
            />
            {/* L4: the copy this chat works in */}
            {multi && active && (
              <ChatContextBar info={active.info} onDiscard={() => {
                  setDiscardFromClose(false)
                  setDiscarding(active.id)
                }} onOpenInEditor={openInEditor} />
            )}
            <RestartBanner
              state={state}
              canRestart={platform.can.restart}
              onRestart={restartEngine}
            />
            {signIn ? (
              signInPanel
            ) : (
              <Transcript
                items={state.items}
                busy={state.busy}
                folded={settings.foldTools}
                showReasoning={settings.showReasoning}
                onLink={openLink}
                onAnswer={(id, text) => controller.answer(id, text)}
                selectedTool={toolKey}
                onSelectTool={(item) => {
                  setToolKey(item.key)
                  openPane('changes')
                }}
                problemActions={{
                  onSignIn: () => controller.slash('login'),
                  onRetry: state.unsent && !state.busy ? () => composer.current?.submit() : undefined,
                  onOpenLink: openLink,
                }}
                empty={
                  <div className="empty">
                    <PixelLogo pixel={6} />
                    <h2>What should Neuron do in {project?.name ?? 'this project'}?</h2>
                    <div className="starters">
                      {STARTERS.map((starter) => (
                        <button key={starter} disabled={!ready || untrusted} onClick={() => controller.send(starter)}>
                          {starter}
                        </button>
                      ))}
                    </div>
                    <p className="muted small">@ adds a file · / runs a command · Ctrl+N new session · Ctrl+K commands · Ctrl+B side panel</p>
                  </div>
                }
              />
            )}
            {!signIn && <TodoStrip todos={state.todos} />} {/* L1: live transcript */}
            <Composer
              key={active?.id} // L4: a chat switch never sends one chat's draft or queue to another
              ref={composer}
              busy={state.busy}
              disabled={!ready || untrusted}
              disabledReason={ready && untrusted ? 'Allow Neuron in this folder to start' : undefined}
              questionWaiting={question !== null}
              enterSends={settings.enterSends}
              searchFiles={platform.searchFiles}
              restore={state.unsent ?? refusedPrompt}
              sessionId={state.status?.session_id}
              turnFailed={state.items.at(-1)?.kind === 'error'}
              effortPending={effortPending}
              features={state.features}
              supportsImages={state.status?.supports_images === true}
              stageAttachment={platform.stageAttachment}
              store={platform.store}
              projectPath={folder}
              permissionWaiting={permission !== null}
              onCommand={runCommand}
              onCycleMode={cycleMode}
              onSend={(text, attachments) => {
                setToolKey(null)
                controller.send(text, attachments)
              }}
              onInterrupt={() => controller.interrupt()}
              toolbar={
                <>
                  <ModeMenu
                    mode={state.status?.permission_mode ?? 'workspace-write'}
                    planMode={state.status?.plan_mode ?? false}
                    disabled={!ready || state.busy}
                    onMode={applyMode}
                    onPlan={applyPlan}
                  />
                  {/* The browser build does not apply launch options. */}
                  {platform.kind !== 'host' && <EffortPicker value={effort} pending={effortPending} disabled={!ready} onChange={changeEffort} />}
                  <ModelPicker
                    current={state.status?.model ?? ''}
                    currentName={state.status?.model_display_name}
                    currentProvider={state.status?.provider}
                    models={state.models}
                    disabled={!ready || state.busy}
                    openSignal={modelSignal}
                    onOpen={() => controller.listModels()}
                    onPick={(model) => controller.setModel(model)}
                  />
                </>
              }
            />
            {exitedProblem && (
              <EngineProblemCard
                problem={exitedProblem}
                actions={{ onRestart: platform.can.restart ? restartEngine : undefined, onCopyDiagnostics: copyDiagnostics, onOpenLink: openLink }}
              />
            )}
            {state.connection === 'exited' && !exitedProblem && (
              <div className="error-line" role="alert">
                {state.connectionError ?? 'The Neuron engine stopped.'}
                {platform.can.restart && (
                  <button className="link-button" onClick={restartEngine}>
                    Restart it
                  </button>
                )}
              </div>
            )}
          </>
        )}
      </main>
      {paneOpen && view === 'chat' && (
        <SidePane
          platform={platform}
          tab={paneTab}
          onTab={(tab) => {
            if (tab === 'changes') setToolKey(null)
            openPane(tab)
          }}
          refreshKey={refreshKey}
          changed={new Set(changes)}
          tool={paneTab === 'changes' ? tool : null}
          todos={state.todos}
          subagents={runningSubagents(state)}
          terminalOpened={terminalOpened}
          state={state}
          controller={controller}
          chatKey={active?.id}
        />
      )}
      <StatusBar state={state} onPane={openPane} />
      {permission && (
        <PermissionDialog
          key={permission.id}
          pending={permission}
          cwd={state.status?.cwd}
          onAnswer={(decision) => controller.answerPermission(permission.id, decision)}
        />
      )}
      {firstRun && (
        <FirstRun
          needsSignIn={signIn}
          signIn={signInPanel}
          current={project}
          recent={recent}
          canPickFolder={platform.can.pickFolder}
          onPickFolder={() => platform.pickFolder()}
          onOpenFolder={(path) => openFolder(path)}
          onDone={() => updateSettings({ firstRunDone: true })}
        />
      )}
      {!firstRun && untrusted && !signIn && folder && (
        <TrustModal
          folder={folder}
          preview={state.trustPreview}
          working={trusting}
          canPickFolder={platform.can.pickFolder}
          onAllow={() => {
            setTrusting(true)
            trustedOnce.current = state.trustPreview?.session_only === true
            controller.trustWorkspace()
          }}
          onElsewhere={() => void pickFolder()}
          onOpenLink={openLink}
        />
      )}
      {signingOut === 'confirm' && (
        <SignOutDialog
          email={account.email}
          onCancel={() => setSigningOut(null)}
          onConfirm={() => {
            controller.slash('logout')
            setSigningOut('restart')
            setView('chat')
          }}
        />
      )}
      {/* L4: multi-chat */}
      <CommandPalette open={palette} onClose={() => setPalette(false)} commands={paletteCommands} />
      <ShortcutsSheet open={shortcutsOpen} onClose={() => setShortcutsOpen(false)} />
      {discarding && multi && platform.chats && discardItem?.info.worktree && (
        <DiscardDialog
          key={discarding}
          info={discardItem.info}
          notice={discardFromClose ? 'This chat works in its own copy. Discard the copy to close the chat, or keep it.' : undefined}
          loadStatus={() => platform.chats!.worktreeStatus(discarding)}
          discard={async (options) => {
            await platform.chats!.discardWorktree(discarding, options)
            await closeChat(discarding)
          }}
          onClose={() => setDiscarding(null)}
        />
      )}
      {toast && (
        <div className="toast" role="status">
          <span>{toast}</span>
          <button className="icon-button" aria-label="Dismiss" onClick={() => setToast(null)}>
            ✕
          </button>
        </div>
      )}
    </div>
  )
}
