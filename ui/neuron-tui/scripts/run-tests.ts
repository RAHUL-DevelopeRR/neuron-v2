// Runs `bun test` over the unit tests in src/, skipping *.integration.test.*.
// Replaces Freebuff's `find`-based test:files script so it also works on
// Windows. Extra arguments are passed through to `bun test`.
const files = [...new Bun.Glob('src/**/*.test.{ts,tsx}').scanSync('.')]
  .map((file) => file.replace(/\\/g, '/'))
  .filter((file) => !/\.integration\.test\.tsx?$/.test(file))
  .sort()
  .map((file) => `./${file}`)

const result = Bun.spawnSync(
  [process.execPath, 'test', ...process.argv.slice(2), ...files],
  { stdio: ['inherit', 'inherit', 'inherit'] },
)
process.exit(result.exitCode ?? 1)
