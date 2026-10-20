// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
import { initializeThemeStore } from '../hooks/use-theme'
import { setProjectRoot } from '../project-files'
import { initTimestampFormatter } from '../utils/helpers'
import { enableManualThemeRefresh } from '../utils/theme-system'
import { initializeDirenv } from './init-direnv'

export async function initializeApp(params: { cwd?: string }): Promise<void> {
  if (params.cwd) {
    process.chdir(params.cwd)
  }
  const baseCwd = process.cwd()
  setProjectRoot(baseCwd)

  // Initialize direnv environment before anything else
  initializeDirenv()

  initializeThemeStore()
  enableManualThemeRefresh()
  initTimestampFormatter()
}
