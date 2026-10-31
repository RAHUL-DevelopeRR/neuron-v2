// A DOM for component tests. Bun's own WebSocket is kept for tests that talk to a real host.
import { GlobalRegistrator } from '@happy-dom/global-registrator'

const BunWebSocket = globalThis.WebSocket
GlobalRegistrator.register({ url: 'http://127.0.0.1:1420/' })
;(globalThis as { BunWebSocket?: typeof WebSocket }).BunWebSocket = BunWebSocket
