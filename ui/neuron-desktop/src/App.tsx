import { useCallback, useEffect, useMemo, useState } from 'react'

import {
  FEATURE_EXTENSIONS,
  FEATURE_SESSIONS,
  FEATURE_TRUST_WORKSPACE,
  changedPaths,
  hasFeature,
  needsSignIn,
  openQuestionItem,
  runningSubagents,
  type SessionController,
  type TranscriptItem,
} from '@neuron/client'

import { ChatHeader, StatusBar, TrustBanner } from './components/Bars'
import { Composer, ModeMenu } from './components/Composer'
import { ProjectDialog, SettingsDialog } from './components/Dialogs'
import { ModelPicker } from './components/ModelPicker'
import { PermissionDialog } from './components/PermissionDialog'
import { PixelLogo } from './components/PixelLogo'
import { SidePane, type PaneTab } from './components/SidePane'
import { Sidebar } from './components/Sidebar'
import { SignInPanel } from './components/SignIn'
import { Transcript } from './components/Transcript'
import { useSession } from './hooks'
import type { Platform, Project } from './platform'
import { useSettings } from './settings'

type ToolItem = Extract<TranscriptItem, { kind: 'tool' }>

const STARTERS = [
  'Explain how this project is organised',
  'Find likely bugs in the code I changed recently',
  'Write tests for the least-tested module',
]

export function App({ platform }: { platform: Platform }) {
  const [controller, setController] = useState<SessionController | null>(null)
  const [launchError, setLaunchError] = useState<string | null>(null)
  const state = useSession(controller)
  const [settings, updateSettings] = useSettings()
  const [paneOpen, setPaneOpen] = useState(true)
  const [paneTab, setPaneTab] = useState<PaneTab>('changes')
  const [terminalOpened, setTerminalOpened] = useState(false)
  const [toolKey, setToolKey] = useState<string | null>(null)
  const [dialog, setDialog] = useState<'project' | 'settings' | null>(null)
  const [recent, setRecent] = useState<Project[]>([])

  useEffect(() => {
    let cancelled = false
    platform.connect().then(
      (connected) => !cancelled && setController(connected),
      (error: Error) => !cancelled && setLaunchError(error.message),
    )
    return () => {
      cancelled = true
    }
  }, [platform])

  const history = hasFeature(state, FEATURE_SESSIONS)
  const ready = state.connection === 'ready'
  const sessionId = state.status?.session_id || null

  // The chat list follows the engine: after it starts, after every turn, and on a switch.
  useEffect(() => {
    if (controller && ready && history && !state.busy) controller.listSessions()
  }, [controller, ready, history, state.busy, sessionId])

  const openPane = useCallback((tab: PaneTab) => {
    if (tab === 'terminal') setTerminalOpened(true)
    setPaneTab(tab)
    setPaneOpen(true)
  }, [])

  const refreshExtensions = useCallback(() => {
    if (ready) controller?.listExtensions()
  }, [controller, ready])

  const newChat = useCallback(() => {
    if (!controller || state.busy) return
    setToolKey(null)
    controller.newChat()
  }, [controller, state.busy])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey)) return
      if (event.key === '`') {
        event.preventDefault()
        if (paneOpen && paneTab === 'terminal') setPaneOpen(false)
        else openPane('terminal')
      } else if (event.key.toLowerCase() === 'b') {
        event.preventDefault()
        setPaneOpen((open) => !open)
      } else if (event.key.toLowerCase() === 'n') {
        event.preventDefault()
        newChat()
      } else if (event.key === ',') {
        event.preventDefault()
        setDialog('settings')
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [paneOpen, paneTab, openPane, newChat])

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
  const project = platform.project()
  const title =
    state.sessions?.find((session) => session.id === sessionId)?.title ??
    (state.items.find((item) => item.kind === 'user') as Extract<TranscriptItem, { kind: 'user' }> | undefined)?.text.split('\n')[0] ??
    'New chat'

  if (!controller) {
    return (
      <div className="splash">
        <PixelLogo pixel={10} />
        {launchError ? (
          <div className="error-line" role="alert">
            {launchError}
          </div>
        ) : (
          <div className="muted">Starting the Neuron engine…</div>
        )}
      </div>
    )
  }

  return (
    <div className={`app${paneOpen ? '' : ' pane-closed'}`}>
      <Sidebar
        project={project}
        sessions={state.sessions}
        currentId={sessionId}
        busy={state.busy}
        historyAvailable={history}
        canOpenProject={platform.can.openProject}
        onNewChat={newChat}
        onResume={(id) => {
          setToolKey(null)
          controller.resume(id)
        }}
        onOpenProject={() => {
          void platform.recentProjects().then(setRecent, () => setRecent([]))
          setDialog('project')
        }}
        onSettings={() => setDialog('settings')}
      />
      <main className="chat">
        <ChatHeader state={state} title={title} where={platform.label} sidePaneOpen={paneOpen} onToggleSidePane={() => setPaneOpen(!paneOpen)} />
        <TrustBanner
          state={state}
          canTrust={hasFeature(state, FEATURE_TRUST_WORKSPACE)}
          canRestart={platform.can.restart}
          onTrust={() => controller.trustWorkspace()}
          onRestart={() => void platform.restart(sessionId).catch((error: Error) => setLaunchError(error.message))}
        />
        {signIn ? (
          <SignInPanel
            code={state.signInCode}
            starting={state.sessionStarting}
            busy={state.busy}
            onSignIn={() => controller.slash('login')}
            onCancel={() => controller.interrupt()}
            onOpen={(url) => platform.openExternal(url)}
          />
        ) : (
          <Transcript
            items={state.items}
            busy={state.busy}
            folded={settings.foldTools}
            showReasoning={settings.showReasoning}
            onLink={(url) => platform.openExternal(url)}
            onAnswer={(id, text) => controller.answer(id, text)}
            selectedTool={toolKey}
            onSelectTool={(item) => {
              setToolKey(item.key)
              openPane('changes')
            }}
            empty={
              <div className="empty">
                <PixelLogo pixel={6} />
                <h2>What should Neuron do in {project?.name ?? 'this project'}?</h2>
                <div className="starters">
                  {STARTERS.map((starter) => (
                    <button key={starter} disabled={!ready} onClick={() => controller.send(starter)}>
                      {starter}
                    </button>
                  ))}
                </div>
                <p className="muted small">@ adds a file · / runs a command · Ctrl+N new chat · Ctrl+B side panel</p>
              </div>
            }
          />
        )}
        <Composer
          busy={state.busy}
          disabled={!ready}
          questionWaiting={question !== null}
          enterSends={settings.enterSends}
          searchFiles={platform.searchFiles}
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
                onMode={(mode) => controller.command('permissions', mode)}
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
      </main>
      {paneOpen && (
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
          extensions={hasFeature(state, FEATURE_EXTENSIONS) ? state.extensions : undefined}
          onRefreshExtensions={refreshExtensions}
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
      {dialog === 'project' && (
        <ProjectDialog current={project} recent={recent} onOpen={(path) => platform.openProject(path)} onClose={() => setDialog(null)} />
      )}
      {dialog === 'settings' && (
        <SettingsDialog
          settings={settings}
          onChange={updateSettings}
          onClose={() => setDialog(null)}
          version={state.version}
          where={platform.label}
        />
      )}
    </div>
  )
}
