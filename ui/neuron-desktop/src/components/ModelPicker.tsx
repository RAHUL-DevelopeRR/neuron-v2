import { useEffect, useRef, useState } from 'react'

import type { ModelInfo } from '@neuron/client'

import { modelLabel, modelRowName } from '../modelNames'

/**
 * The model chip and its menu. The list comes from `list_models` (only the
 * Zero-X gateway publishes one); a model id can always be typed. Models are
 * named without the route the gateway takes to them; a pick still sends the id.
 */
export function ModelPicker({
  current,
  currentName,
  currentProvider,
  models,
  disabled,
  onOpen,
  onPick,
  openSignal = 0,
}: {
  current: string
  /** The engine's own name for `current` (`status.model_display_name`), when it sent one. */
  currentName?: string | null
  /** Who serves `current` (`status.provider`). */
  currentProvider?: string | null
  models: ModelInfo[]
  disabled: boolean
  onOpen: () => void
  onPick: (model: string) => void
  /** L3: counts up each time `/model` asks for the menu. */
  openSignal?: number
}) {
  const [open, setOpen] = useState(false)
  const [typed, setTyped] = useState('')
  const box = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const close = (event: MouseEvent) => {
      if (!box.current?.contains(event.target as Node)) setOpen(false)
    }
    window.addEventListener('mousedown', close)
    return () => window.removeEventListener('mousedown', close)
  }, [open])
  useEffect(() => {
    if (openSignal === 0) return
    onOpen()
    setOpen(true)
    // Only a new signal opens it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openSignal])
  const shown = modelLabel(current, models, { name: currentName, provider: currentProvider })

  return (
    <div className="model-picker" ref={box}>
      <button
        className="model-chip"
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        title="Switch model"
        onClick={() => {
          if (!open) onOpen()
          setOpen(!open)
        }}
      >
        {shown || 'model'}
      </button>
      {open && (
        <div className="menu up" role="listbox" aria-label="Models">
          {models.map((model) => {
            const unusable = !model.available || model.status === 'blurred'
            const why =
              model.unavailable_reason ?? (model.status === 'blurred' ? `back ${model.available_again_at ?? 'soon'}` : null)
            const detail = why ?? [model.status && model.status !== 'available' ? model.status : null, model.tiers.join(', ')].filter(Boolean).join(' · ')
            return (
              <button
                key={model.id}
                role="option"
                aria-selected={model.id === current}
                className={`menu-item${model.id === current ? ' current' : ''}`}
                disabled={unusable}
                onClick={() => {
                  onPick(model.id)
                  setOpen(false)
                }}
              >
                <span>{modelRowName(models, model)}</span>
                <span className="muted">{detail}</span>
              </button>
            )
          })}
          <form
            className="menu-typed"
            onSubmit={(event) => {
              event.preventDefault()
              if (!typed.trim()) return
              onPick(typed.trim())
              setTyped('')
              setOpen(false)
            }}
          >
            <input
              aria-label="Model id"
              placeholder={models.length ? 'Or type a model id' : 'Model id, e.g. sonnet'}
              value={typed}
              onChange={(event) => setTyped(event.target.value)}
            />
          </form>
        </div>
      )}
    </div>
  )
}
