// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron: the check
// is named `isAtOrAboveHome`, which the git-root banner uses too.
// See ui/neuron-tui/PORTING.md.
import os from 'os'
import path from 'path'

/**
 * Whether `dir` is the home folder or a folder above it: too broad a place to
 * work in, which the engine refuses.
 */
export function isAtOrAboveHome(dir: string, homeDir: string = os.homedir()): boolean {
  const relativeToHome = path.relative(dir, homeDir)
  return (
    relativeToHome === '' ||
    (!relativeToHome.startsWith('..') && !path.isAbsolute(relativeToHome))
  )
}

/**
 * Whether the UI starts by asking for a project: from the home folder or above
 * it, unless it was opened to continue a session (`neuron --resume`), which
 * belongs to the folder it was started in and would be lost by moving.
 */
export function shouldShowProjectPicker(
  startCwd: string,
  homeDir: string,
  resumingSession = false,
): boolean {
  return !resumingSession && isAtOrAboveHome(startCwd, homeDir)
}
