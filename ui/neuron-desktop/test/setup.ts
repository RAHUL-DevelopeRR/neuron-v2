// A DOM for component tests. Bun's own WebSocket is kept for tests that talk to a real host.
import { GlobalRegistrator } from '@happy-dom/global-registrator'

const BunWebSocket = globalThis.WebSocket
// The size the app's window opens at (tauri.conf.json), so the sidebar is full width.
GlobalRegistrator.register({ url: 'http://127.0.0.1:1420/', width: 1400, height: 900 })
;(globalThis as { BunWebSocket?: typeof WebSocket }).BunWebSocket = BunWebSocket
