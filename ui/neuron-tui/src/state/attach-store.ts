import { create } from 'zustand'

/** A line the attachments banner shows about a file that was not attached. */
export interface AttachNotice {
  id: number
  text: string
}

interface AttachState {
  /** The in-UI file browser, in the panel slot, when no OS dialog can open. */
  pickerOpen: boolean
  /** The folder the browser starts in. */
  pickerStartDir: string | null
  /** An OS file dialog is open. */
  dialogOpen: boolean
  notices: AttachNotice[]
  openPicker: (startDir: string) => void
  closePicker: () => void
  setDialogOpen: (open: boolean) => void
  addNotice: (text: string) => number
  removeNotice: (id: number) => void
  clearNotices: () => void
  reset: () => void
}

let nextNoticeId = 1

export const useAttachStore = create<AttachState>()((set) => ({
  pickerOpen: false,
  pickerStartDir: null,
  dialogOpen: false,
  notices: [],
  openPicker: (startDir) => set({ pickerOpen: true, pickerStartDir: startDir }),
  closePicker: () => set({ pickerOpen: false }),
  setDialogOpen: (open) => set({ dialogOpen: open }),
  addNotice: (text) => {
    const id = nextNoticeId++
    set((state) => ({ notices: [...state.notices, { id, text }] }))
    return id
  },
  removeNotice: (id) => set((state) => ({ notices: state.notices.filter((notice) => notice.id !== id) })),
  clearNotices: () => set({ notices: [] }),
  reset: () => set({ pickerOpen: false, pickerStartDir: null, dialogOpen: false, notices: [] }),
}))
