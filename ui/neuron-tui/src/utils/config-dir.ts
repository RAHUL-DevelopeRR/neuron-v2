// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
import os from 'os'
import path from 'path'

import { getCliEnv } from './env'

/**
 * Resolve the on-disk config directory for the UI: settings, chat history and
 * logs. Defaults to ~/.neuron/tui, beside the engine's own ~/.neuron files.
 * (Freebuff used ~/.config/manicode, which Codebuff itself still writes.)
 *
 * Lives in its own module (depending only on `env`) so low-level helpers can
 * read the config dir without import cycles.
 */
export const getConfigDir = (): string => {
  const configuredDir = getCliEnv().NEURON_TUI_CONFIG_DIR
  if (configuredDir) {
    if (!path.isAbsolute(configuredDir)) {
      throw new Error(
        'NEURON_TUI_CONFIG_DIR must be an absolute path so CLI settings cannot be written relative to the current project.',
      )
    }
    return configuredDir
  }

  return path.join(os.homedir(), '.neuron', 'tui')
}
