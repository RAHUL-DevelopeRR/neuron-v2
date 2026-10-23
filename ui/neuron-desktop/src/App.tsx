import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import {
  FEATURE_EXTENSIONS,
  FEATURE_SESSIONS,
  FEATURE_TRUST_WORKSPACE,
  accountView,
  changedPaths,
  hasFeature,
  needsSignIn,
  openQuestionItem,
  runningSubagents,
  type SessionController,
  type TranscriptItem,
} from '@neuron/client'

import { AccountRow, SignOutDialog, type AccountAction } from './components/AccountMenu'
import { ChatHeader, RestartBanner, StatusBar } from './components/Bars'
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
import { useSession } from './hooks'
import { LINKS } from './links'
import type { LaunchOptions, Platform, Project, UpdateInfo } from './platform'
import { noteSeen, sessionRows, type SessionRow } from './sessions'
import { useSeen, useSettings, type Settings } from './settings'

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
  const [controller, setController] = useState<SessionController | null>(null)
  const [launchError, setLaunchError] = useState<string | null>(null)
  const state = useSession(controller)
  const [settings, updateSettings, settingsLoaded] = useSettings(platform.store)
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

  // The engine starts once the settings say how.
  useEffect(() => {
    if (!settingsLoaded) return
    let cancelled = false
    platform.setLaunchOptions(launchOptions(settings, null))
    platform.connect().then(
      (connected) => !cancelled && setController(connected),
      (error: Error) => !cancelled && setLaunchError(error.message),
    )
    return () => {
      cancelled = true
    }
    // Settings changed later apply to the next engine, not this one.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [platform, settingsLoaded])

  // The version, and whether a newer release exists, once per launch.
  useEffect(() => {
    void platform.appVersion().then(setAppVersion, () => {})
    if (platform.can.updates) void platform.checkForUpdate().then(setUpdate, () => {})
  }, [platform])

  const refreshRecent = useCallback(() => {
    void platform.recentProjects().then(setRecent, () => setRecent([]))
  }, [platform])
  useEffect(() => refreshRecent(), [refreshRecent])

  const history = hasFeature(state, FEATURE_SESSIONS)
  const ready = state.connection === 'ready'
  const sessionId = state.status?.session_id || null
  const project = platform.project()
  const folder = state.status?.cwd ?? project?.path ?? null

  // The session list follows the engine: after it starts, after every turn, and on a switch.
  useEffect(() => {
    if (controller && ready && history && !state.busy) controller.listSessions()
  }, [controller, ready, history, state.busy, sessionId])

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

  const openPane = useCallback((tab: PaneTab) => {
    if (tab === 'terminal') setTerminalOpened(true)
    setPaneTab(tab)
    setPaneOpen(true)
  }, [])

  const refreshExtensions = useCallback(() => {
    if (ready) controller?.listExtensions()
  }, [controller, ready])

  const newSession = useCallback(() => {
    if (!controller || state.busy) return
    setToolKey(null)
    setView('chat')
    controller.newChat()
  }, [controller, state.busy])

  const openFolder = useCallback(
    async (path: string, resume: string | null = null) => {
      setSwitching(true)
      setProjectError(null)
      try {
        platform.setLaunchOptions(launchOptions(settings, path))
        modeApplied.current = path
        await platform.openProject(path, resume)
        setToolKey(null)
        setView('chat')
        refreshRecent()
      } catch (failure) {
        setProjectError(failure instanceof Error ? failure.message : String(failure))
        throw failure
      } finally {
        setSwitching(false)
      }
    },
    [platform, settings, refreshRecent],
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
      if (!row.project.path || row.project.path === project?.path) controller.resume(row.id)
      else void openFolder(row.project.path, row.id).catch(() => {})
    },
    [controller, project, openFolder],
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

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && view !== 'chat' && !document.querySelector('[aria-modal="true"], [role="menu"]')) {
        setView('chat')
        return
      }
      if (!(event.ctrlKey || event.metaKey)) return
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
      } else if (key === 'n') {
        event.preventDefault()
        newSession()
      } else if (key === 'k') {
        event.preventDefault()
        focusSearch()
      } else if (event.key === ',') {
        event.preventDefault()
        openSettings()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [paneOpen, paneTab, openPane, newSession, focusSearch, openSettings, rail, view])

  // Signing out: the engine forgets the sign-in, then a fresh engine shows the sign-in screen.
  useEffect(() => {
    if (signingOut === 'restart' && !state.busy) {
      setSigningOut(null)
      void platform.restart(sessionId).catch((error: Error) => setLaunchError(error.message))
    }
  }, [signingOut, state.busy, platform, sessionId])

  // Once allowed, the folder's own settings load in a fresh engine, on the same session.
  useEffect(() => {
    if (!trusting || !state.restartRequired) return
    setTrusting(false)
    if (platform.can.restart) {
      const keep = state.items.some((item) => item.kind === 'user') ? sessionId : null
      void platform.restart(keep).catch((error: Error) => setLaunchError(error.message))
    }
  }, [trusting, state.restartRequired, state.items, platform, sessionId])

  const changes = useMemo(() => changedPaths(state), [state])
  // Re-read the tree, git and open files whenever a call finishes.
  const refreshKey = useMemo(
    () => state.items.filter((item) => item.kind === 'tool' && item.state !== 'running' && item.state !== 'awaiting_permission').length,
    [state.items],
  )
  const tool = (state.items.find((item) => item.key === toolKey) as ToolItem | undefined) ?? null
  const permission = state.permissions[0] ?? null
  const signIn = needsSignIn(state) || state.signInCode !== null || state.sessionStarting
  const question = openQuestionItem(state)
  const account = accountView(state.status?.signed_in, state.status?.account)
  const untrusted =
    hasFeature(state, FEATURE_TRUST_WORKSPACE) && state.status !== null && !state.status.workspace_trusted && !state.restartRequired
  const firstRun = settingsLoaded && !settings.firstRunDone && controller !== null
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
        {launchError ? (
          <div className="error-line" role="alert">
            {launchError}
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
        rows={history ? rows : null}
        open={project}
        groupBy={settings.groupBy}
        onGroupBy={(groupBy) => updateSettings({ groupBy })}
        busy={state.busy || switching}
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
            busy={state.busy || switching}
            error={projectError}
            onPick={() => void pickFolder()}
            onOpen={(path) => void openFolder(path).catch(() => {})}
            onClose={() => setView('chat')}
          />
        )}
        {view === 'sessions' && (
          <AllSessionsPage rows={rows ?? []} groupBy={settings.groupBy} busy={state.busy || switching} onOpen={openSession} onClose={() => setView('chat')} />
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
                    <p className="muted small">@ adds a file · / runs a command · Ctrl+N new session · Ctrl+K search · Ctrl+B side panel</p>
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
          gitRoot={null}
          preview={[]}
          working={trusting}
          canPickFolder={platform.can.pickFolder}
          onAllow={() => {
            setTrusting(true)
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
    </div>
  )
}
