// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the folder
// icons come from utils/glyphs.ts, the logo is in NeuronCLI's brand colours,
// and "New folder", "No project" and the Ctrl shortcuts are Neuron's own.
// See ui/neuron-tui/PORTING.md.
import os from 'os'
import path from 'path'

import React, { useCallback, useMemo, useState } from 'react'

import { Button } from './button'
import { MultilineInput } from './multiline-input'
import { SelectableList } from './selectable-list'
import { TerminalLink } from './terminal-link'
import { useDirectoryBrowser } from '../hooks/use-directory-browser'
import { useLogo } from '../hooks/use-logo'
import { usePathTabCompletion } from '../hooks/use-path-tab-completion'
import { useSearchableList } from '../hooks/use-searchable-list'
import { useSheenAnimation } from '../hooks/use-sheen-animation'
import { useTerminalLayout } from '../hooks/use-terminal-layout'
import { useTheme } from '../hooks/use-theme'
import { exitCliCleanly } from '../utils/exit-cleanly'
import { GLYPHS } from '../utils/glyphs'
import { formatCwd } from '../utils/path-helpers'
import {
  createNewFolder,
  createScratchWorkspace,
  projectPickerKeyAction,
  SCRATCH_PARENT,
  shortName,
} from '../utils/project-picker'
import { loadRecentProjects } from '../utils/recent-projects'
import { isPlainEnterKey } from '../utils/terminal-enter-detection'

import type { SelectableListItem } from './selectable-list'

// Layout constants for responsive breakpoints
const LAYOUT = {
  // Content width constraints
  MAX_CONTENT_WIDTH: 80,
  PREFERRED_CONTENT_WIDTH: 60,
  CONTENT_PADDING: 4,

  // Essential element heights (always shown)
  INPUT_HEIGHT: 1,
  HINT_HEIGHT: 1, // shortcuts, or the new folder's prompt and errors
  BOTTOM_BAR_HEIGHT: 2,
  // "New folder" (14) + "No project" (14) + "Open" (10) + the gaps between
  // them and the path (3): what is left of the bar shows the path.
  BOTTOM_BUTTONS_WIDTH: 41,
  MIN_LIST_HEIGHT: 2, // Minimum rows to show in file picker
  MAX_LIST_HEIGHT: 12,

  // Compact mode threshold - below this, remove padding/margins
  COMPACT_MODE_THRESHOLD: 12,

  // Decorative element heights
  LOGO_HEIGHT: 8,
  HELP_TEXT_HEIGHT: 2,

  // Spacing constants (used in normal mode)
  MAIN_CONTENT_PADDING: 2,
  LOGO_MARGIN_TOP: 1,
  LOGO_MARGIN_BOTTOM: 1,
  HELP_TEXT_MARGIN_BOTTOM: 1,
  RECENTS_MARGIN_TOP: 1,
  RECENTS_PADDING_LEFT: 1,
} as const

/** How a folder was chosen: `scratch` is the "No project" folder. */
export interface ProjectSelectOptions {
  scratch?: boolean
}

/** The shortcuts, shown under the filter. */
export const PICKER_SHORTCUTS_HINT = 'Ctrl+N new folder  Ctrl+O open  Ctrl+P/Alt+P no project'

type PickerNotice = { kind: 'error' | 'info'; text: string }

/** The end of `text` in `max` columns: the folder's own name is the useful part. */
export function truncateStart(text: string, max: number): string {
  if (text.length <= max) return text
  if (max <= 3) return text.slice(text.length - Math.max(0, max))
  return `...${text.slice(text.length - (max - 3))}`
}

interface ProjectPickerScreenProps {
  /** Called when user selects a directory to open as project */
  onSelectProject: (projectPath: string, options?: ProjectSelectOptions) => void
  /** Initial path to browse from */
  initialPath?: string
  /** The home folder "No project" makes its scratch folder under (tests). */
  homeDir?: string
}

export const ProjectPickerScreen: React.FC<ProjectPickerScreenProps> = ({
  onSelectProject,
  initialPath,
  homeDir = os.homedir(),
}) => {
  const theme = useTheme()
  const [sheenPosition, setSheenPosition] = useState(0)

  // Directory browsing state and navigation
  const {
    currentPath,
    setCurrentPath,
    directories,
    expandPath,
    tryNavigateToPath,
    navigateToDirectory,
  } = useDirectoryBrowser({ initialPath })

  // Convert directories to SelectableListItem format
  const directoryItems: SelectableListItem[] = useMemo(
    () =>
      directories.map((entry) => ({
        id: entry.path,
        label: entry.name,
        icon: entry.isParent ? GLYPHS.folderOpen : GLYPHS.folder,
        accent: entry.isGitRepo,
      })),
    [directories],
  )

  // Search filtering and focus management
  const {
    searchQuery,
    setSearchQuery,
    focusedIndex,
    setFocusedIndex,
    filteredItems: filteredDirectoryItems,
    handleFocusChange,
  } = useSearchableList({
    items: directoryItems,
    resetKey: currentPath,
  })

  // "New folder": the filter input takes the new folder's name while naming.
  const [naming, setNaming] = useState(false)
  const [newFolderName, setNewFolderName] = useState('')
  const [notice, setNotice] = useState<PickerNotice | null>(null)

  // Load recent projects, excluding the home directory
  const recentProjects = useMemo(
    () => loadRecentProjects().filter((project) => project.path !== homeDir),
    [homeDir],
  )

  // Use the terminal layout hook for responsive breakpoints
  const { terminalWidth, terminalHeight } = useTerminalLayout()
  const contentMaxWidth = Math.min(
    terminalWidth - LAYOUT.CONTENT_PADDING,
    LAYOUT.MAX_CONTENT_WIDTH,
  )
  const contentWidth = Math.min(LAYOUT.PREFERRED_CONTENT_WIDTH, contentMaxWidth)

  // Compact mode: remove padding/margins when space is tight
  const isCompactMode = terminalHeight < LAYOUT.COMPACT_MODE_THRESHOLD
  const mainPadding = isCompactMode ? 0 : LAYOUT.MAIN_CONTENT_PADDING

  // Calculate essential height first (these always show)
  // Essential = input (1) + file picker border (2) + bottom bar (2) + minimal padding
  const essentialHeight =
    LAYOUT.INPUT_HEIGHT +
    LAYOUT.HINT_HEIGHT +
    2 +
    LAYOUT.BOTTOM_BAR_HEIGHT +
    (isCompactMode ? 0 : 2)

  // Calculate remaining height for file picker and optional elements
  const remainingHeight = terminalHeight - essentialHeight

  // File picker gets priority - calculate how much space it needs
  const filePickerHeight = Math.max(
    LAYOUT.MIN_LIST_HEIGHT,
    Math.min(remainingHeight, LAYOUT.MAX_LIST_HEIGHT),
  )

  // After file picker, calculate space for optional elements
  const spaceAfterFilePicker = remainingHeight - filePickerHeight

  // Determine which optional elements can fit (priority: recents first, then logo, then help text)
  const logoHeightNeeded =
    LAYOUT.LOGO_HEIGHT +
    (isCompactMode ? 0 : LAYOUT.LOGO_MARGIN_TOP + LAYOUT.LOGO_MARGIN_BOTTOM)
  const helpTextHeightNeeded =
    LAYOUT.HELP_TEXT_HEIGHT +
    (isCompactMode ? 0 : LAYOUT.HELP_TEXT_MARGIN_BOTTOM)

  // Allocate space for optional elements based on available space
  let availableForOptional = spaceAfterFilePicker

  // Try to fit recents first (most useful)
  let recentsToShow = 0
  if (recentProjects.length > 0 && availableForOptional >= 2) {
    // Calculate how many recents fit
    const baseRecentsHeight =
      1 + (isCompactMode ? 0 : LAYOUT.RECENTS_MARGIN_TOP) // header + margin
    const remainingForRecents = availableForOptional - baseRecentsHeight
    recentsToShow = Math.min(
      recentProjects.length,
      Math.max(0, remainingForRecents),
      3,
    )
    if (recentsToShow > 0) {
      availableForOptional -=
        recentsToShow + 1 + (isCompactMode ? 0 : LAYOUT.RECENTS_MARGIN_TOP)
    }
  }

  // Try to fit logo (decorative but nice)
  const canShowLogo = !isCompactMode && availableForOptional >= logoHeightNeeded
  if (canShowLogo) {
    availableForOptional -= logoHeightNeeded
  }

  // Try to fit help text (least important)
  const canShowHelpText =
    !isCompactMode && availableForOptional >= helpTextHeightNeeded

  const canShowRecents = recentsToShow > 0
  const maxRecentsToShow = recentsToShow

  // File picker is always shown if there's any space
  const canShowFilePicker = remainingHeight >= LAYOUT.MIN_LIST_HEIGHT
  const maxListHeight = filePickerHeight

  // Center content only in non-compact mode when there's extra space
  const shouldCenterContent = !isCompactMode && spaceAfterFilePicker > 10

  // Logo setup
  const { applySheenToChar } = useSheenAnimation({
    themeName: theme.name,
    terminalWidth,
    sheenPosition,
    setSheenPosition,
  })

  const { component: logoComponent } = useLogo({
    availableWidth: contentMaxWidth,
    applySheenToChar,
    textColor: theme.foreground,
    themeName: theme.name,
  })

  const cancelNaming = useCallback(() => {
    setNaming(false)
    setNewFolderName('')
    setNotice(null)
  }, [])

  // Handle directory selection from SelectableList. A click while naming
  // moves elsewhere, so the name (not yet created) is dropped, as Esc would.
  const handleDirectorySelect = useCallback(
    (item: SelectableListItem) => {
      if (naming) cancelNaming()
      const entry = directories.find((d) => d.path === item.id)
      if (entry) {
        navigateToDirectory(entry)
      }
    },
    [naming, cancelNaming, directories, navigateToDirectory],
  )

  // Select current directory as project
  const selectCurrentDirectory = useCallback(() => {
    onSelectProject(currentPath)
  }, [currentPath, onSelectProject])

  const startNaming = useCallback(() => {
    setNaming(true)
    setNewFolderName('')
    setNotice(null)
  }, [])

  // Create the named folder here and move into it, so Open opens it. Returns
  // the new folder, or null when the name was refused (the notice says why).
  const confirmNewFolder = useCallback((): string | null => {
    const result = createNewFolder(newFolderName, currentPath)
    if (!result.ok) {
      setNotice({ kind: 'error', text: result.error })
      return null
    }
    setNaming(false)
    setNewFolderName('')
    setSearchQuery('')
    setCurrentPath(result.path)
    setNotice({
      kind: 'info',
      text: `Created "${shortName(path.basename(result.path), 18)}". Press Ctrl+O to open it.`,
    })
    return result.path
  }, [newFolderName, currentPath, setSearchQuery, setCurrentPath])

  // The Open button. While naming, it creates the named folder and opens it,
  // rather than opening the folder the new one was to go in.
  const handleOpenClick = useCallback(() => {
    if (!naming) {
      selectCurrentDirectory()
      return
    }
    const made = confirmNewFolder()
    if (made) onSelectProject(made)
  }, [naming, selectCurrentDirectory, confirmNewFolder, onSelectProject])

  // "No project": a scratch folder under ~/Neuron, never the home folder.
  const startWithoutProject = useCallback(() => {
    if (naming) cancelNaming()
    const result = createScratchWorkspace(homeDir)
    if (!result.ok) {
      setNotice({ kind: 'error', text: result.error })
      return
    }
    onSelectProject(result.path, { scratch: true })
  }, [naming, cancelNaming, homeDir, onSelectProject])

  // Tab completion for path input
  const { handleTabCompletion } = usePathTabCompletion({
    searchQuery,
    setSearchQuery,
    currentPath,
    setCurrentPath,
    expandPath,
  })

  // Handle search input keyboard intercept
  const handleSearchKeyIntercept = useCallback(
    (key: {
      name?: string
      sequence?: string
      shift?: boolean
      ctrl?: boolean
      meta?: boolean
      option?: boolean
    }) => {
      // Ctrl+C always quits
      if (key.name === 'c' && key.ctrl) {
        void exitCliCleanly()
        return true
      }
      if (naming) {
        if (key.name === 'escape') {
          cancelNaming()
          return true
        }
        if (isPlainEnterKey(key)) {
          confirmNewFolder()
          return true
        }
        // The list and the shortcuts wait until the name is in or cancelled.
        if (
          key.name === 'up' ||
          key.name === 'down' ||
          key.name === 'tab' ||
          projectPickerKeyAction(key) !== 'none'
        ) {
          return true
        }
        return false
      }
      const action = projectPickerKeyAction(key)
      if (action !== 'none') {
        if (action === 'new_folder') startNaming()
        else if (action === 'open') selectCurrentDirectory()
        else startWithoutProject()
        return true
      }
      if (notice) setNotice(null)
      if (key.name === 'escape') {
        if (searchQuery.length > 0) {
          setSearchQuery('')
        }
        return true
      }
      if (key.name === 'tab') {
        return handleTabCompletion()
      }
      if (key.name === 'up') {
        setFocusedIndex((prev) => Math.max(0, prev - 1))
        return true
      }
      if (key.name === 'down') {
        setFocusedIndex((prev) =>
          Math.min(filteredDirectoryItems.length - 1, prev + 1),
        )
        return true
      }
      if (isPlainEnterKey(key)) {
        // If search looks like a path, try to navigate there directly
        if (searchQuery.startsWith('/') || searchQuery.startsWith('~')) {
          if (tryNavigateToPath(searchQuery)) {
            return true
          }
        }
        // Otherwise, navigate to the focused directory
        const focused = filteredDirectoryItems[focusedIndex]
        if (focused) {
          const entry = directories.find((d) => d.path === focused.id)
          if (entry) {
            navigateToDirectory(entry)
          }
        }
        return true
      }
      // All other single-character keys should go to the input for typing
      return false
    },
    [
      naming,
      notice,
      cancelNaming,
      confirmNewFolder,
      startNaming,
      selectCurrentDirectory,
      startWithoutProject,
      searchQuery,
      setSearchQuery,
      handleTabCompletion,
      setFocusedIndex,
      filteredDirectoryItems,
      focusedIndex,
      tryNavigateToPath,
      directories,
      navigateToDirectory,
    ],
  )

  const scratchParentLabel = formatCwd(path.join(homeDir, SCRATCH_PARENT))
  const secondaryButtonStyle = {
    paddingLeft: 1,
    paddingRight: 1,
    paddingTop: 0,
    paddingBottom: 0,
    borderStyle: 'single',
    borderColor: theme.muted,
    flexShrink: 0,
  }

  return (
    <box
      style={{
        width: '100%',
        height: '100%',
        backgroundColor: theme.surface,
        padding: 0,
        flexDirection: 'column',
      }}
    >
      {/* Main content area - fills available space */}
      <box
        style={{
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: shouldCenterContent ? 'center' : 'flex-start',
          width: '100%',
          padding: mainPadding,
          gap: isCompactMode ? 0 : 1,
          flexGrow: 1,
          flexShrink: 1,
        }}
      >
        {/* Logo - show when there's enough space after essentials */}
        {canShowLogo && (
          <box
            style={{
              flexDirection: 'column',
              alignItems: 'center',
              width: '100%',
              marginTop: isCompactMode ? 0 : LAYOUT.LOGO_MARGIN_TOP,
              marginBottom: isCompactMode ? 0 : LAYOUT.LOGO_MARGIN_BOTTOM,
              flexShrink: 0,
            }}
          >
            <box style={{ flexDirection: 'column', alignItems: 'flex-start' }}>
              {logoComponent}
            </box>
          </box>
        )}

        {/* Help text - show only when there's plenty of space */}
        {canShowHelpText && (
          <box
            style={{
              flexDirection: 'column',
              alignItems: 'center',
              maxWidth: contentMaxWidth,
              marginBottom: isCompactMode ? 0 : LAYOUT.HELP_TEXT_MARGIN_BOTTOM,
              flexShrink: 0,
            }}
          >
            <text style={{ fg: theme.muted, wrapMode: 'word' }}>
              Navigate to your project folder and press Open.
            </text>
            <text style={{ fg: theme.muted, wrapMode: 'word' }}>
              {`No project starts in a scratch folder in ${scratchParentLabel}.`}
            </text>
          </box>
        )}

        {/* Search/filter input - always visible, high priority */}
        <box
          style={{
            width: contentWidth,
            flexShrink: 0,
            marginBottom: 0,
          }}
        >
          <MultilineInput
            value={naming ? newFolderName : searchQuery}
            onChange={({ text }) => {
              if (naming) {
                setNewFolderName(text)
                if (notice) setNotice(null)
              } else {
                setSearchQuery(text)
              }
            }}
            onSubmit={() => {}} // Enter key handled by onKeyIntercept
            onPaste={() => {}} // Paste not needed for path input
            onKeyIntercept={handleSearchKeyIntercept}
            placeholder={
              naming ? 'Name of the new folder...' : 'Select project directory...'
            }
            focused={true}
            maxHeight={1}
            minHeight={1}
            cursorPosition={naming ? newFolderName.length : searchQuery.length}
          />
        </box>

        {/* Shortcuts, or the new folder's prompt and what went wrong */}
        <box style={{ width: contentWidth, height: 1, flexShrink: 0, overflow: 'hidden' }}>
          <text
            style={{
              fg:
                notice?.kind === 'error'
                  ? theme.error
                  : notice?.kind === 'info'
                    ? theme.success
                    : theme.muted,
              wrapMode: 'none',
            }}
          >
            {notice
              ? notice.text
              : naming
                ? 'Enter creates the folder here, Esc cancels.'
                : PICKER_SHORTCUTS_HINT}
          </text>
        </box>

        {/* Directory list - only show if we have enough space */}
        {canShowFilePicker && (
          <box
            style={{
              flexDirection: 'column',
              width: contentWidth,
              borderStyle: 'single',
              borderColor: theme.muted,
              flexShrink: 0,
            }}
            border={['top', 'bottom', 'left', 'right']}
          >
            <SelectableList
              items={filteredDirectoryItems}
              focusedIndex={focusedIndex}
              maxHeight={maxListHeight}
              onSelect={handleDirectorySelect}
              onFocusChange={handleFocusChange}
              emptyMessage={
                searchQuery ? 'No matching directories' : 'No subdirectories'
              }
            />
          </box>
        )}

        {/* Recent Projects - show when there's space after file picker */}
        {canShowRecents && (
          <box
            style={{
              flexDirection: 'column',
              width: contentWidth,
              marginTop: isCompactMode ? 0 : LAYOUT.RECENTS_MARGIN_TOP,
              flexShrink: 0,
              gap: 0,
            }}
          >
            <text style={{ fg: theme.muted, height: 1 }}>Recent:</text>
            {recentProjects.slice(0, maxRecentsToShow).map((project, idx) => (
              <box
                key={project.path}
                style={{
                  flexDirection: 'row',
                  gap: 1,
                  paddingLeft: isCompactMode ? 0 : LAYOUT.RECENTS_PADDING_LEFT,
                  height: 1,
                }}
              >
                <text style={{ fg: theme.secondary }}>[{idx + 1}]</text>
                <TerminalLink
                  text={formatCwd(project.path)}
                  onActivate={() => onSelectProject(project.path)}
                  underlineOnHover={true}
                  containerStyle={{ width: 'auto' }}
                />
              </box>
            ))}
          </box>
        )}
      </box>

      {/* Bottom bar - fixed at bottom with New folder, No project and Open */}
      <box
        style={{
          flexDirection: 'row',
          alignItems: 'center',
          justifyContent: 'center',
          width: '100%',
          paddingTop: 0,
          paddingBottom: 0,
          borderStyle: 'single',
          borderColor: theme.border,
          flexShrink: 0,
          backgroundColor: theme.surface,
        }}
        border={['top']}
      >
        <box
          style={{
            flexDirection: 'row',
            alignItems: 'center',
            justifyContent: 'space-between',
            width: contentWidth,
            gap: 1,
          }}
        >
          {/* Current directory path */}
          <box style={{ flexGrow: 1, flexShrink: 1, overflow: 'hidden' }}>
            <text style={{ fg: theme.muted, wrapMode: 'none' }}>
              {truncateStart(
                formatCwd(currentPath),
                contentWidth - LAYOUT.BOTTOM_BUTTONS_WIDTH,
              )}
            </text>
          </box>

          <Button
            onClick={startNaming}
            style={secondaryButtonStyle}
            border={['top', 'bottom', 'left', 'right']}
          >
            <text style={{ fg: theme.foreground }}>New folder</text>
          </Button>

          <Button
            onClick={startWithoutProject}
            style={secondaryButtonStyle}
            border={['top', 'bottom', 'left', 'right']}
          >
            <text style={{ fg: theme.foreground }}>No project</text>
          </Button>

          {/* Open button */}
          <Button
            onClick={handleOpenClick}
            style={{
              paddingLeft: 2,
              paddingRight: 2,
              paddingTop: 0,
              paddingBottom: 0,
              borderStyle: 'single',
              borderColor: theme.primary,
              backgroundColor: theme.primary,
              flexShrink: 0,
            }}
            border={['top', 'bottom', 'left', 'right']}
          >
            <text style={{ fg: '#1a1a1a' }}>Open</text>
          </Button>
        </box>
      </box>
    </box>
  )
}
