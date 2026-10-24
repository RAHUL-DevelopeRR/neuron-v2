import type { Diff } from '@neuron/client'

export function DiffView({ diff, label }: { diff: Diff; label?: string }) {
  return (
    <div className="diff" aria-label={label ?? 'diff'}>
      <div className="diff-summary">
        <span className="diff-added">+{diff.added}</span> <span className="diff-removed">−{diff.removed}</span>
        {diff.truncated && <span className="muted"> (cut)</span>}
      </div>
      <table>
        <tbody>
          {diff.lines.map((line, index) =>
            line.kind === 'hunk' ? (
              <tr key={index} className="diff-hunk">
                <td colSpan={3}>{line.text}</td>
              </tr>
            ) : (
              <tr key={index} className={`diff-${line.kind}`} data-kind={line.kind}>
                <td className="diff-number">{line.oldNumber ?? ''}</td>
                <td className="diff-number">{line.newNumber ?? ''}</td>
                <td className="diff-text">
                  <span className="diff-sign">{line.kind === 'add' ? '+' : line.kind === 'remove' ? '−' : ' '}</span>
                  {line.text}
                </td>
              </tr>
            ),
          )}
        </tbody>
      </table>
    </div>
  )
}
