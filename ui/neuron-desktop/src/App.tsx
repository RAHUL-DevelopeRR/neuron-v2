import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import {
  FEATURE_ACCOUNT,
  FEATURE_EXTENSIONS,
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
import { Composer, ModeMenu, type ComposerHandle } from './components/Composer'
import { ExtensionsPanel } from './components/ExtensionsPanel'
import { FirstRun } from './components/FirstRun'
import { ModelPicker } from './components/ModelPicker'
import { AllSessionsPage, ExtensionsPage, ProjectsPage, UpdateNotice } from './components/Pages'
import { PermissionDialog } from './components/PermissionDialog'
import { PixelLogo } from './components/PixelLogo'
import { SettingsPage, type SettingsSection, type UpdateCheck } from './components/SettingsPage'
import { SidePane, type PaneTab } from './components/SidePane'
import { Sidebar, type NavTarget } from './components/Sidebar'
import { SignInPanel } from './components/SignIn'
import { TrustModal } from './components/TrustModal'
import { Transcript } from './components/Transcript'
import { useChats } from './chats'
import { useSession } from './hooks'
import { LINKS } from './links'
import type { LaunchOptions, Platform, Project, UpdateInfo } from './platform'
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
function launchOptions(settings: Settings, folder: string | null): LaunchOptions {
  const remembered = settings.rememberModePerFolder && folder ? settings.modeByFolder[folder] : undefined
  return {
    model: settings.defaultModel.trim() || undefined,
    permissionMode: remembered ?? settings.defaultPermissionMode,
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
  const [discarding, setDiscarding] = useState(false)
  // The engines start once the settings say how.
  useEffect(() => {
    if (settingsLoaded) platform.setLaunchOptions(launchOptions(settings, null))
    // Settings changed later apply to the next engine, not this one.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [platform, settingsLoaded])
  const { store: chats, snapshot, active, launchError } = useChats(platform, settingsLoaded, setToast)
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
  const search = useRef<HTMLInputElement>(null)
  const composer = useRef<ComposerHandle>(null)
  const narrow = useNarrow()

  // The version, and whether a newer release exists, once per launch.
  useEffect(() => {
    void platform.appVersion().then(setAppVersion, () => {})
    if (platform.can.updates && !platform.manualUpdates) void platform.checkForUpdate().then(setUpdate, () => {})
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
  const wasBusy = useRef(false)
  useEffect(() => {
    if (wasBusy.current && !state.busy && sessionId) {
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
    wasBusy.current = state.busy
  }, [state.busy, state.items, sessionId, updateSeen])

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
  // Opening a file in the editor belongs to the review lane; the app wires it where it exists.
  const openInEditor: (() => void) | undefined = undefined

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
      if (!chats) return
      setSwitching(true)
      try {
        await chats.open(request)
        setToolKey(null)
        setView('chat')
      } catch (failure) {
        setToast(failure instanceof Error ? failure.message : String(failure))
        throw failure
      } finally {
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
      setSwitching(true)
      setProjectError(null)
      try {
        platform.setLaunchOptions(launchOptions(settings, path))
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
      else setShortcutsOpen(true)
    },
    [newSession, newWorktreeChat, chats],
  )

  // The handler reads the latest actions, so a key pressed as the chats appear is not lost.
  const shortcutAction = useRef(runShortcut)
  shortcutAction.current = runShortcut

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && view !== 'chat' && !document.querySelector('[aria-modal="true"], [role="menu"]')) {
        setView('chat')
        return
      }
      if (!(event.ctrlKey || event.metaKey)) return
      const shortcut = shortcutFor(event)
      if (shortcut) {
        event.preventDefault()
        shortcutAction.current(shortcut)
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
  const closeChat = useCallback(
    async (id: string) => {
      if (!chats) return
      await chats.close(id)
      // A window always has a chat: closing the last one opens a fresh one.
      if (chats.getSnapshot().items.length === 0) await openChat({ path: project?.path }).catch(() => {})
    },
    [chats, openChat, project],
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
  const firstRun = settingsLoaded && !settings.firstRunDone && controller !== null

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

  const openLink = (url: string) => platform.openExternal(url)
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

  if (!controller) {
    return (
      <div className="splash">
        <PixelLogo pixel={10} />
        {(launchError ?? restartError) ? (
          <div className="error-line" role="alert">
            {launchError ?? restartError}
          </div>
        ) : (
          <div className="muted" role="status">
            Starting the Neuron engine…
          </div>
        )}
      </div>
    )
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
              <ExtensionsPanel extensions={state.extensions} onRefresh={refreshExtensions} />
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
              <ChatContextBar info={active.info} onDiscard={() => setDiscarding(true)} onOpenInEditor={openInEditor} />
            )}
            <RestartBanner
              state={state}
              canRestart={platform.can.restart}
              onRestart={() => void platform.restart(sessionId).catch((error: Error) => setLaunchError(error.message))}
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
            <Composer
              ref={composer}
              busy={state.busy}
              disabled={!ready || untrusted}
              disabledReason={ready && untrusted ? 'Allow Neuron in this folder to start' : undefined}
              questionWaiting={question !== null}
              enterSends={settings.enterSends}
              searchFiles={platform.searchFiles}
              restore={state.unsent}
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
                    onMode={(mode) => {
                      controller.command('permissions', mode)
                      if (settings.rememberModePerFolder && folder) updateSettings({ modeByFolder: { ...settings.modeByFolder, [folder]: mode } })
                    }}
                    onPlan={(on) => controller.command('plan', on ? 'on' : 'off')}
                  />
                  <ModelPicker
                    current={state.status?.model ?? ''}
                    models={state.models}
                    disabled={!ready || state.busy}
                    onOpen={() => controller.listModels()}
                    onPick={(model) => controller.setModel(model)}
                  />
                </>
              }
            />
            {state.connection === 'exited' && (
              <div className="error-line" role="alert">
                {state.connectionError ?? 'The Neuron engine stopped.'}
                {platform.can.restart && (
                  <button className="link-button" onClick={() => void platform.restart(sessionId).catch(() => {})}>
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
      {discarding && multi && active?.info.worktree && platform.chats && (
        <DiscardDialog
          info={active.info}
          loadStatus={() => platform.chats!.worktreeStatus(active.id)}
          discard={async (options) => {
            await platform.chats!.discardWorktree(active.id, options)
            await closeChat(active.id)
          }}
          onClose={() => setDiscarding(false)}
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
