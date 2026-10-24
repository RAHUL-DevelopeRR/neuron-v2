/** Keyboard and screen-reader helpers for dialogs and menus. */

import { useEffect, useRef, type RefObject } from 'react'

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'

export function focusables(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((element) => !element.hasAttribute('hidden'))
}

/**
 * While `active`: focus moves into `ref` (its `[data-autofocus]` element, or
 * the first focusable one), Tab and Shift+Tab stay inside, and on close focus
 * goes back to where it was.
 */
export function useFocusTrap(ref: RefObject<HTMLElement | null>, active = true): void {
  useEffect(() => {
    const root = ref.current
    if (!active || !root) return
    const before = document.activeElement as HTMLElement | null
    const first = root.querySelector<HTMLElement>('[data-autofocus]') ?? focusables(root)[0] ?? root
    first.focus()
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Tab') return
      const items = focusables(root)
      if (items.length === 0) {
        event.preventDefault()
        return
      }
      const index = items.indexOf(document.activeElement as HTMLElement)
      const next = event.shiftKey ? (index <= 0 ? items.length - 1 : index - 1) : index === items.length - 1 ? 0 : index + 1
      event.preventDefault()
      items[next].focus()
    }
    root.addEventListener('keydown', onKey)
    return () => {
      root.removeEventListener('keydown', onKey)
      if (before && document.contains(before)) before.focus()
    }
  }, [ref, active])
}

/** Arrow keys, Home and End move between a menu's items; Escape and Tab close it. */
export function useMenuKeys(ref: RefObject<HTMLElement | null>, open: boolean, onClose: () => void): void {
  const close = useRef(onClose)
  close.current = onClose
  useEffect(() => {
    const root = ref.current
    if (!open || !root) return
    const items = () => [...root.querySelectorAll<HTMLElement>('[role="menuitem"]:not([disabled])')]
    items()[0]?.focus()
    const onKey = (event: KeyboardEvent) => {
      const list = items()
      const index = list.indexOf(document.activeElement as HTMLElement)
      if (event.key === 'ArrowDown') list[(index + 1) % list.length]?.focus()
      else if (event.key === 'ArrowUp') list[(index - 1 + list.length) % list.length]?.focus()
      else if (event.key === 'Home') list[0]?.focus()
      else if (event.key === 'End') list[list.length - 1]?.focus()
      else if (event.key === 'Escape' || event.key === 'Tab') {
        if (event.key === 'Escape') event.preventDefault()
        close.current()
        return
      } else return
      event.preventDefault()
    }
    root.addEventListener('keydown', onKey)
    return () => root.removeEventListener('keydown', onKey)
  }, [ref, open])
}
