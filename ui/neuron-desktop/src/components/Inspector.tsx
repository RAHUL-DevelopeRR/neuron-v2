import { useEffect, useState } from 'react'

import type { TranscriptItem } from '@neuron/client'

import type { FileContent, Platform } from '../platform'
import { DiffView } from './DiffView'

type ToolItem = Extract<TranscriptItem, { kind: 'tool' }>

/** The side panel: the files this session changed, and the file or tool call picked. */
export function Inspector({
  platform,
  changes,
  file,
  tool,
  refreshKey,
  onOpenFile,
}: {
  platform: Platform
  changes: string[]
  file: string | null
  tool: ToolItem | null
  refreshKey: number
  onOpenFile: (path: string) => void
}) {
  const [content, setContent] = useState<FileContent | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (!file) return
    let cancelled = false
    platform.read(file).then(
      (result) => !cancelled && (setContent(result), setError(null)),
      (failure: Error) => !cancelled && (setContent(null), setError(failure.message)),
    )
    return () => {
      cancelled = true
    }
  }, [platform, file, refreshKey])

  return (
    <aside className="inspector" aria-label="Details">
      <div className="pane-title">Changes</div>
      {changes.length === 0 ? (
        <div className="muted pad">No file changes yet</div>
      ) : (
        <ul className="changes">
          {changes.map((path) => (
            <li key={path}>
              <button className={`tree-row changed${file === path ? ' selected' : ''}`} onClick={() => onOpenFile(path)}>
                {path}
              </button>
            </li>
          ))}
        </ul>
      )}
      {tool && !file && (
        <section className="inspector-body">
          <div className="pane-title">
            {tool.view.verb} <span className="muted">{tool.view.target}</span>
          </div>
          {tool.view.diff && <DiffView diff={tool.view.diff} />}
          {tool.view.output && <pre className="tool-output">{tool.view.output}</pre>}
        </section>
      )}
      {file && (
        <section className="inspector-body">
          <div className="pane-title">{file}</div>
          {error && <div className="error-line small">{error}</div>}
          {content?.binary && <div className="muted pad">Binary file ({content.size} bytes)</div>}
          {content && !content.binary && (
            <pre className="file-view" data-testid="file-view">
              {content.content}
              {content.truncated && '\n… (cut)'}
            </pre>
          )}
        </section>
      )}
    </aside>
  )
}
