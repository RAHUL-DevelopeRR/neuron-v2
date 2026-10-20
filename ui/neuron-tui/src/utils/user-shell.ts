/**
 * Runs a shell command the user typed in bash mode (`!cmd`).
 *
 * Freebuff ran these through the Codebuff SDK's terminal runner (with a
 * console-free broker process on Windows). That runner is gone with the SDK;
 * until Neuron decides whether `!` commands run here or in the engine, this
 * fails with a message the bash bubble shows.
 */

export interface UserShellResult {
  stdout: string
  stderr: string
  exitCode: number
}

export class UserShellUnavailableError extends Error {
  constructor() {
    super('Shell commands (!) are not available in this build of the Neuron UI yet.')
    this.name = 'UserShellUnavailableError'
  }
}

export async function runUserShellCommand(_params: {
  command: string
  cwd: string
}): Promise<UserShellResult> {
  throw new UserShellUnavailableError()
}
