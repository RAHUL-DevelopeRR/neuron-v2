import fs from 'fs'
import path from 'path'

import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// In a browser, the app attaches to a Neuron host (`bun run host --mock` in
// ui/neuron-client). Its URL comes from NEURON_HOST_URL and its pairing token
// from the file NEURON_HOST_TOKEN_FILE names, so the token is never typed or
// printed. Inside Tauri neither is used.
const tokenFile = process.env.NEURON_HOST_TOKEN_FILE
const hostToken = tokenFile && fs.existsSync(tokenFile) ? fs.readFileSync(tokenFile, 'utf8').trim() : ''

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  resolve: {
    alias: { '@neuron/client': path.resolve(__dirname, '../neuron-client/src/index.ts') },
  },
  define: {
    'import.meta.env.VITE_NEURON_HOST_URL': JSON.stringify(process.env.NEURON_HOST_URL ?? ''),
    'import.meta.env.VITE_NEURON_HOST_TOKEN': JSON.stringify(hostToken),
  },
  server: {
    port: Number(process.env.NEURON_DESKTOP_PORT ?? 1420),
    strictPort: true,
    host: '127.0.0.1',
    fs: { allow: [path.resolve(__dirname, '..')] },
  },
  build: { target: 'es2022', outDir: 'dist', emptyOutDir: true },
})
