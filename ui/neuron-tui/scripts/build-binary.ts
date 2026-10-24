#!/usr/bin/env bun
// Derived from Codebuff/Freebuff cli/scripts/build-binary.ts (Apache-2.0);
// modified for Neuron: no bundled-agent or SDK prebuild, no tree-sitter wasm
// and no NEXT_PUBLIC_* service env baked in (Neuron's UI talks only to its
// local engine). See ui/neuron-tui/PORTING.md.
//
// Compiles the UI into one executable: `bun run build:binary <version>`
// writes bin/neuron-tui (bin/neuron-tui.exe on Windows) for this machine.
// OVERRIDE_TARGET (a Bun target such as bun-darwin-x64), OVERRIDE_PLATFORM and
// OVERRIDE_ARCH build for another one; --outfile=<path> picks the output.

import { spawnSync, type SpawnSyncOptions } from 'child_process'
import { createRequire } from 'module'
import { chmodSync, mkdirSync, readFileSync, realpathSync, rmSync } from 'fs'
import { dirname, join, resolve } from 'path'
import { fileURLToPath } from 'url'

import {
  ensureOpenTuiNativeBundle,
  type OpenTuiNativeTarget,
} from './open-tui-native-bundle'

type TargetInfo = {
  bunTarget: string
  platform: NodeJS.Platform
  arch: string
}

const VERBOSE = process.env.VERBOSE === 'true'
const OVERRIDE_TARGET = process.env.OVERRIDE_TARGET
const OVERRIDE_PLATFORM = process.env.OVERRIDE_PLATFORM as NodeJS.Platform | undefined
const OVERRIDE_ARCH = process.env.OVERRIDE_ARCH ?? undefined
const OVERRIDE_COMPILE_EXECUTABLE_PATH = process.env.BUN_COMPILE_EXECUTABLE_PATH

const cliRoot = join(dirname(fileURLToPath(import.meta.url)), '..')
const cliRequire = createRequire(join(cliRoot, 'package.json'))

const log = (message: string) => {
  if (VERBOSE) console.log(message)
}

function runCommand(command: string, args: string[], options: SpawnSyncOptions = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd,
    stdio: VERBOSE ? 'inherit' : 'pipe',
    env: options.env,
  })
  if (result.status !== 0) {
    const stderr = result.stderr?.toString() ?? ''
    throw new Error(
      `Command "${command} ${args.join(' ')}" failed with exit code ${result.status}${stderr ? `\n${stderr}` : ''}`,
    )
  }
}

/** The Bun target for this machine (or the OVERRIDE_* one). */
export function getTargetInfo(
  platform: string = process.platform,
  arch: string = process.arch,
): TargetInfo {
  if (OVERRIDE_TARGET && OVERRIDE_PLATFORM && OVERRIDE_ARCH) {
    return { bunTarget: OVERRIDE_TARGET, platform: OVERRIDE_PLATFORM, arch: OVERRIDE_ARCH }
  }
  const mappings: Record<string, TargetInfo> = {
    'linux-x64': { bunTarget: 'bun-linux-x64', platform: 'linux', arch: 'x64' },
    'linux-arm64': { bunTarget: 'bun-linux-arm64', platform: 'linux', arch: 'arm64' },
    'darwin-x64': { bunTarget: 'bun-darwin-x64', platform: 'darwin', arch: 'x64' },
    'darwin-arm64': { bunTarget: 'bun-darwin-arm64', platform: 'darwin', arch: 'arm64' },
    'win32-x64': { bunTarget: 'bun-windows-x64', platform: 'win32', arch: 'x64' },
  }
  const target = mappings[`${platform}-${arch}`]
  if (!target) throw new Error(`Unsupported build target: ${platform}-${arch}`)
  return target
}

function targetLabel(targetInfo: TargetInfo): string {
  const base = `${targetInfo.platform}-${targetInfo.arch}`
  return targetInfo.bunTarget.endsWith('-baseline') ? `${base}-baseline` : base
}

async function main() {
  const args = process.argv.slice(2)
  const outArg = args.find((arg) => arg.startsWith('--outfile='))
  const version = args.find((arg) => !arg.startsWith('--'))
  if (!version) {
    throw new Error('Usage: bun run build:binary <version> [--outfile=<path>]')
  }

  const targetInfo = getTargetInfo()
  const outputFile = outArg
    ? resolve(outArg.slice('--outfile='.length))
    : join(cliRoot, 'bin', targetInfo.platform === 'win32' ? 'neuron-tui.exe' : 'neuron-tui')
  mkdirSync(dirname(outputFile), { recursive: true })

  prepareOpenTuiNativeBundle(targetInfo)

  const defineFlags = [
    ['process.env.NODE_ENV', '"production"'],
    ['process.env.NEURON_TUI_IS_BINARY', '"true"'],
    ['process.env.NEURON_TUI_VERSION', `"${version}"`],
    ['process.env.NEURON_TUI_TARGET', `"${targetLabel(targetInfo)}"`],
  ]

  const buildArgs = [
    'build',
    'src/entry.ts',
    '--compile',
    // The production JSX runtime (the dev one crashes compiled binaries).
    '--production',
    // A project's bunfig.toml or .env must not configure the UI: compiled
    // Bun executables read both from the working directory by default.
    '--no-compile-autoload-bunfig',
    '--no-compile-autoload-dotenv',
    `--target=${targetInfo.bunTarget}`,
    ...(OVERRIDE_COMPILE_EXECUTABLE_PATH
      ? [`--compile-executable-path=${OVERRIDE_COMPILE_EXECUTABLE_PATH}`]
      : []),
    `--outfile=${outputFile}`,
    '--sourcemap=none',
    ...defineFlags.flatMap(([key, value]) => ['--define', `${key}=${value}`]),
  ]
  log(`bun ${buildArgs.join(' ')}`)
  runCommand(process.execPath, buildArgs, { cwd: cliRoot, env: process.env })

  // Bun writes a source map beside the output even with --sourcemap=none.
  rmSync(join(dirname(outputFile), 'entry.js.map'), { force: true })
  if (targetInfo.platform !== 'win32') chmodSync(outputFile, 0o755)
  console.log(`Built ${outputFile} (${targetLabel(targetInfo)}, ${version})`)
}

/**
 * Every OpenTUI native package a build for this target has to RESOLVE, not
 * just the one it loads: `@opentui/core` picks its native module with static
 * `import()`s in a platform switch, and `bun build --compile` resolves every
 * branch it can see, including Linux's musl sibling.
 */
function openTuiNativeTargets(targetInfo: TargetInfo): OpenTuiNativeTarget[] {
  return targetInfo.platform === 'linux'
    ? [targetInfo, { ...targetInfo, libc: 'musl' }]
    : [targetInfo]
}

function openTuiNativePackageFolder(target: OpenTuiNativeTarget): string {
  return `core-${target.platform}-${target.arch}${target.libc === 'musl' ? '-musl' : ''}`
}

function prepareOpenTuiNativeBundle(targetInfo: TargetInfo) {
  const packageJson = JSON.parse(readFileSync(join(cliRoot, 'package.json'), 'utf8')) as {
    dependencies?: Record<string, string>
  }
  const expectedCoreVersion = packageJson.dependencies?.['@opentui/core']
  const expectedReactVersion = packageJson.dependencies?.['@opentui/react']
  if (!expectedCoreVersion || !expectedReactVersion) {
    throw new Error('package.json must pin @opentui/core and @opentui/react')
  }
  const corePackage = getInstalledOpenTuiPackage('core', expectedCoreVersion)
  // Resolve both up front so a stale split install fails before the build.
  void getInstalledOpenTuiPackage('react', expectedReactVersion)

  const packagesDir = dirname(corePackage.packageDir)
  const registry = process.env.NPM_REGISTRY_URL

  for (const target of openTuiNativeTargets(targetInfo)) {
    const packageFolder = openTuiNativePackageFolder(target)
    const packageName = `@opentui/${packageFolder}`
    const version = corePackage.packageJson.optionalDependencies?.[packageName]
    if (version !== expectedCoreVersion) {
      throw new Error(`Installed OpenTUI core does not declare ${packageName}@${expectedCoreVersion}`)
    }
    const result = ensureOpenTuiNativeBundle({
      packageDir: join(packagesDir, packageFolder),
      version,
      targetInfo: target,
      installBundle: (stagingRoot) => {
        runCommand(
          process.execPath,
          [
            'install',
            '--cwd',
            stagingRoot,
            '--no-save',
            // The musl package has the same os/cpu as its glibc sibling.
            `--os=${targetInfo.platform}`,
            `--cpu=${targetInfo.arch}`,
            ...(registry ? [`--registry=${registry}`] : []),
            `${packageName}@${version}`,
          ],
          { env: process.env },
        )
      },
    })
    console.log(
      result === 'reused'
        ? `OpenTUI native bundle ${version} already present for ${packageFolder}`
        : `Installed OpenTUI native bundle ${version} for ${packageFolder}`,
    )
  }
}

function getInstalledOpenTuiPackage(packageFolder: 'core' | 'react', expectedVersion: string) {
  const packageName = `@opentui/${packageFolder}`
  let packageDir: string
  try {
    packageDir = dirname(realpathSync(cliRequire.resolve(packageName)))
  } catch {
    throw new Error(`${packageName} is missing; run bun install before building`)
  }
  const packageJson = JSON.parse(readFileSync(join(packageDir, 'package.json'), 'utf8')) as {
    name?: unknown
    version?: unknown
    optionalDependencies?: Record<string, string>
  }
  if (packageJson.name !== packageName || packageJson.version !== expectedVersion) {
    throw new Error(
      `Installed ${packageName}@${String(packageJson.version)} does not match package.json (${expectedVersion}); run bun install`,
    )
  }
  return { packageDir, packageJson }
}

if (import.meta.main) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exit(1)
  })
}

