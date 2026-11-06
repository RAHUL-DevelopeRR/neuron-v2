import { memo, useId, useState } from 'react'

import type { TranscriptItem } from '@neuron/client'

import { CopyButton, Markdown } from './Markdown'

type ReportItem = Extract<TranscriptItem, { kind: 'report' }>

/** The report as plain text, for the clipboard. */
export function reportText(item: ReportItem): string {
  const sections = item.sections.map((section) => (section.heading ? `${section.heading}\n${section.markdown}` : section.markdown))
  return [item.title, ...sections].join('\n\n')
}

/** A `/usage`, `/init`, `/memory` or `/doctor` report: sections as Markdown, links as buttons; collapsible, copyable. */
export const ReportPanel = memo(function ReportPanel({ item, onLink }: { item: ReportItem; onLink?: (url: string) => void }) {
  const [open, setOpen] = useState(true)
  const body = useId()
  return (
    <section className="report" aria-label={`${item.title} report`}>
      <div className="report-head">
        <h2 className="report-title">
          <button className="report-toggle" aria-expanded={open} aria-controls={body} onClick={() => setOpen(!open)}>
            <span aria-hidden>{open ? '▾' : '▸'}</span> {item.title}
          </button>
        </h2>
        <CopyButton text={reportText(item)} label="Copy report" />
      </div>
      {open && (
        <div id={body} className="report-body">
          {item.sections.map((section, index) => (
            <div key={index} className="report-section">
              {section.heading && <h3 className="report-heading">{section.heading}</h3>}
              <Markdown text={section.markdown} onLink={onLink} />
            </div>
          ))}
          {item.links.length > 0 && (
            <div className="report-links">
              {item.links.map((link) => (
                <button key={link.url} className="link-button" onClick={() => onLink?.(link.url)}>
                  {link.label}
                </button>
              ))}
            </div>
          )}
        </div>
      )}
    </section>
  )
})
