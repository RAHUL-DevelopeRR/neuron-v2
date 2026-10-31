// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the
// Freebuff model and catalog picks, reasoning efforts, BYOK selection,
// adsEnabled, Freebucks intro and a-la-carte flags are gone with the services
// they configured, and a saved agent mode is read as DEFAULT while the modes
// are not offered (AGENT_MODES_AVAILABLE). See ui/neuron-tui/PORTING.md.
import fs from 'fs'
import path from 'path'

import { getConfigDir } from './config-dir'
import { AGENT_MODES, AGENT_MODES_AVAILABLE } from './constants'
import { logger } from './logger'

import type { AgentMode } from './constants'

const DEFAULT_SETTINGS: Settings = {
  mode: 'DEFAULT' as const,
}

// Note: The old FREE mode has been renamed back to LITE; migrate on load.

/**
 * Settings schema - add new settings here as the product evolves
 */
export interface Settings {
  mode?: AgentMode
  /** Set once the user has submitted their first prompt. Used to gate the
   *  first-time onboarding suggested prompts so they only show to brand-new
   *  users and quietly retire afterwards. */
  hasSubmittedFirstPrompt?: boolean
}

/**
 * Get the settings file path
 */
export const getSettingsPath = (): string => {
  return path.join(getConfigDir(), 'settings.json')
}

/**
 * Ensure the config directory exists, creating it if necessary
 */
const ensureConfigDirExists = (): void => {
  const configDir = getConfigDir()
  if (!fs.existsSync(configDir)) {
    fs.mkdirSync(configDir, { recursive: true })
  }
}

/**
 * Load all settings from file system
 * @returns The saved settings object, with defaults for missing values
 */
export const loadSettings = (): Settings => {
  const settingsPath = getSettingsPath()

  if (!fs.existsSync(settingsPath)) {
    ensureConfigDirExists()
    // Create default settings file
    fs.writeFileSync(settingsPath, JSON.stringify(DEFAULT_SETTINGS, null, 2))
    return DEFAULT_SETTINGS
  }

  try {
    const settingsFile = fs.readFileSync(settingsPath, 'utf8')
    const parsed = JSON.parse(settingsFile)
    return validateSettings(parsed)
  } catch (error) {
    logger.debug(
      {
        error: error instanceof Error ? error.message : String(error),
      },
      'Error reading settings',
    )
    return {}
  }
}

/**
 * Validate and sanitize settings from file
 */
const validateSettings = (parsed: unknown): Settings => {
  if (typeof parsed !== 'object' || parsed === null) {
    return {}
  }

  const settings: Settings = {}
  const obj = parsed as Record<string, unknown>

  // Validate mode; migrate the previously-saved 'FREE' value to 'LITE'.
  if (AGENT_MODES_AVAILABLE && typeof obj.mode === 'string') {
    const normalized = obj.mode === 'FREE' ? 'LITE' : obj.mode
    if (AGENT_MODES.includes(normalized as AgentMode)) {
      settings.mode = normalized as AgentMode
    }
  }

  // Validate hasSubmittedFirstPrompt
  if (typeof obj.hasSubmittedFirstPrompt === 'boolean') {
    settings.hasSubmittedFirstPrompt = obj.hasSubmittedFirstPrompt
  }

  // The loader is an ALLOWLIST: a key not copied here is dropped on every
  // load and silently rewritten on the next save.
  return settings
}

/**
 * Save settings to file system (merges with existing settings)
 */
export const saveSettings = (newSettings: Partial<Settings>): void => {
  const settingsPath = getSettingsPath()

  try {
    ensureConfigDirExists()

    // Load existing settings and merge
    const existingSettings = loadSettings()
    const mergedSettings = { ...existingSettings, ...newSettings }

    fs.writeFileSync(settingsPath, JSON.stringify(mergedSettings, null, 2))
  } catch (error) {
    logger.debug(
      {
        error: error instanceof Error ? error.message : String(error),
      },
      'Error saving settings',
    )
  }
}

/**
 * Load the saved agent mode preference
 * @returns The saved mode, or 'DEFAULT' if not found or invalid
 */
export const loadModePreference = (): AgentMode => {
  const settings = loadSettings()
  return settings.mode ?? 'DEFAULT'
}

/**
 * Save the agent mode preference
 */
export const saveModePreference = (mode: AgentMode): void => {
  saveSettings({ mode })
}

/**
 * Whether the user has ever submitted a prompt. False only for brand-new
 * users, who get the onboarding suggested prompts on an empty chat.
 */
export const hasSubmittedFirstPrompt = (): boolean => {
  return loadSettings().hasSubmittedFirstPrompt === true
}

/**
 * Mark that the user has submitted their first prompt, retiring the onboarding
 * suggested prompts on future launches. Idempotent.
 */
export const markFirstPromptSubmitted = (): void => {
  if (loadSettings().hasSubmittedFirstPrompt === true) return
  saveSettings({ hasSubmittedFirstPrompt: true })
}
