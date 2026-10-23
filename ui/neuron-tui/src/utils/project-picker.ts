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

export function shouldShowProjectPicker(
  startCwd: string,
  homeDir: string,
): boolean {
  return isAtOrAboveHome(startCwd, homeDir)
}
