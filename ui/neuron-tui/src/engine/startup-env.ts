/**
 * The PATH this process started with, before the direnv import
 * (init/init-direnv.ts) adds a project's `.envrc` to `process.env`. The
 * `neuron` engine is looked up on it, so a repository can't put its own
 * `neuron` first on PATH and have the UI run that as the engine.
 *
 * entry.ts imports this module before anything else, so the value is
 * captured before startup changes the environment.
 */
export const STARTUP_PATH: string | undefined = process.env.PATH ?? process.env.Path
