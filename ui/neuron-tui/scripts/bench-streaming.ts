// Streams a turn through the chat's transcript and status lines and prints
// what it cost: `bun run bench:streaming [tokens] [--json]`. See
// src/testing/streaming-bench.tsx for what is measured. React runs its
// development build here (it reports render times only there), so absolute
// times are higher than in the shipped binary; compare runs with each other.
process.env.NODE_ENV ||= 'development'

const { runStreamingBench } = await import('../src/testing/streaming-bench')

const args = process.argv.slice(2)
const json = args.includes('--json')
const tokens = Number(args.find((arg) => /^\d+$/.test(arg)) ?? 2000)

// One warm-up run so the first-call costs (module init, JIT) stay out of it.
await runStreamingBench({ tokens: 200, historyTurns: 2 })
const result = await runStreamingBench({ tokens })

if (json) {
  console.log(JSON.stringify(result, null, 2))
} else {
  const t = (timing: { meanMs: number; p95Ms: number; maxMs: number; count: number }) =>
    `${timing.count} · mean ${timing.meanMs} ms · p95 ${timing.p95Ms} ms · max ${timing.maxMs} ms`
  console.log(`tokens ${result.tokens}, tool calls ${result.toolCalls}, engine events ${result.engineEvents}, ${Math.round(result.wallMs)} ms wall, ${Math.round(result.cpuMs)} ms CPU`)
  const r = result.renders
  console.log(`transcript   ${result.transcriptUpdates} updates · streaming message drawn ${r.streamingMessage} · earlier messages drawn again ${r.oldMessages}`)
  console.log(`renders      status bar ${r.statusBar} · live activity line ${r.liveActivityLine} · prompt status bar ${r.promptStatusBar}`)
  console.log(`react ms     transcript ${result.reactMs.transcript} · status line ${result.reactMs.statusLine} · prompt status bar ${result.reactMs.promptStatusBar}`)
  console.log(`frames       ${t(result.frames)}`)
  console.log(`event path   ${t(result.eventPath)}`)
  console.log(`loop lag     ${t(result.eventLoopLag)}`)
}
process.exit(0)

export {}
