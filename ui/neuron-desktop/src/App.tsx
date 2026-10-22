import { lazy, Suspense, useEffect, useMemo, useState } from 'react'

import { changedPaths, needsSignIn, openQuestion, type SessionController, type TranscriptItem } from '@neuron/client'

import { StatusBar, TopBar } from './components/Bars'
import { Composer } from './components/Composer'
import { FileTree } from './components/FileTree'
import { Inspector } from './components/Inspector'
import { PermissionDialog } from './components/PermissionDialog'
import { PixelLogo } from './components/PixelLogo'
import { SignInPanel } from './components/SignIn'
import { Transcript } from './components/Transcript'
import { useSession } from './hooks'
import type { Platform } from './platform'

// xterm only loads when the terminal first opens.
const TerminalPane = lazy(() => import('./components/TerminalPane').then((module) => ({ default: module.TerminalPane })))

type ToolItem = Extract<TranscriptItem, { kind: 'tool' }>

export function App({ platform }: { platform: Platform }) {
  const [controller, setController] = useState<SessionController | null>(null)
  const [launchError, setLaunchError] = useState<string | null>(null)
  const state = useSession(controller)
  const [file, setFile] = useState<string | null>(null)
  const [toolKey, setToolKey] = useState<string | null>(null)
  const [terminalOpened, setTerminalOpened] = useState(false)
  const [terminalShown, setTerminalShown] = useState(false)

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

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.ctrlKey && event.key === '`') {
        event.preventDefault()
        setTerminalOpened(true)
        setTerminalShown((shown) => !shown)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const changes = useMemo(() => changedPaths(state), [state])
  // Re-read the tree and the open file whenever a call finishes.
  const refreshKey = useMemo(
    () => state.items.filter((item) => item.kind === 'tool' && item.state !== 'running' && item.state !== 'awaiting_permission').length,
    [state.items],
  )
  const tool = (state.items.find((item) => item.key === toolKey) as ToolItem | undefined) ?? null
  const permission = state.permissions[0] ?? null
  const signIn = needsSignIn(state) || state.signInCode !== null || state.sessionStarting
  const question = openQuestion(state)

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
    <div className={`app${terminalShown ? ' with-terminal' : ''}`}>
      <TopBar
        state={state}
        where={platform.label}
        onListModels={() => controller.listModels()}
        onPickModel={(model) => controller.setModel(model)}
      />
      <FileTree
        platform={platform}
        refreshKey={refreshKey}
        changed={new Set(changes)}
        selected={file}
        onOpen={(path) => {
          setFile(path)
          setToolKey(null)
        }}
      />
      <main className="chat">
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
            onLink={(url) => platform.openExternal(url)}
            selectedTool={toolKey}
            onSelectTool={(item) => {
              setToolKey(item.key)
              setFile(null)
            }}
          />
        )}
        <Composer
          busy={state.busy}
          disabled={state.connection !== 'ready'}
          question={question}
          onSend={(text) => controller.send(text)}
          onInterrupt={() => controller.interrupt()}
        />
        {state.connection === 'exited' && (
          <div className="error-line" role="alert">
            {state.connectionError ?? 'The Neuron engine stopped.'}
          </div>
        )}
      </main>
      <Inspector
        platform={platform}
        changes={changes}
        file={file}
        tool={tool}
        refreshKey={refreshKey}
        onOpenFile={(path) => {
          setFile(path)
          setToolKey(null)
        }}
      />
      <section className="terminal-dock" hidden={!terminalShown} aria-label="Terminal">
        {terminalOpened && (
          <Suspense fallback={<div className="muted pad">Loading the terminal…</div>}>
            <TerminalPane platform={platform} />
          </Suspense>
        )}
      </section>
      <StatusBar
        state={state}
        terminalOpen={terminalShown}
        onToggleTerminal={() => {
          setTerminalOpened(true)
          setTerminalShown(!terminalShown)
        }}
      />
      {permission && (
        <PermissionDialog
          key={permission.id}
          pending={permission}
          cwd={state.status?.cwd}
          onAnswer={(decision) => controller.answerPermission(permission.id, decision)}
        />
      )}
    </div>
  )
}
