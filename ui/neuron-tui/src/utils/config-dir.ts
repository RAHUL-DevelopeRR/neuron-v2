// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
import os from 'os'
import path from 'path'

import { getCliEnv } from './env'

/**
 * Resolve the on-disk config directory for the UI: settings, chat history and
 * logs. In order:
 *   1. NEURON_TUI_CONFIG_DIR, which must be absolute;
 *   2. `tui` in NEURON_HOME, so a relocated Neuron home keeps the UI's files
 *      with the engine's (a relative NEURON_HOME is ignored, as the engine
 *      ignores it for workspace trust, so nothing lands in the project);
 *   3. ~/.neuron/tui, beside the engine's own ~/.neuron files.
 * (Freebuff used ~/.config/manicode, which Codebuff itself still writes.)
 *
 * Lives in its own module (depending only on `env`) so low-level helpers can
 * read the config dir without import cycles.
 */
export const getConfigDir = (): string => {
  const env = getCliEnv()
  const configuredDir = env.NEURON_TUI_CONFIG_DIR
  if (configuredDir) {
    if (!path.isAbsolute(configuredDir)) {
      throw new Error(
        'NEURON_TUI_CONFIG_DIR must be an absolute path so CLI settings cannot be written relative to the current project.',
      )
    }
    return configuredDir
  }

  const neuronHome = env.NEURON_HOME?.trim()
  if (neuronHome && path.isAbsolute(neuronHome)) {
    return path.join(neuronHome, 'tui')
  }

  return path.join(os.homedir(), '.neuron', 'tui')
}
