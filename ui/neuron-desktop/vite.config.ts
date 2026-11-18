import fs from 'fs'
import path from 'path'

import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

import { isUpdaterSigned } from './scripts/updater-signed'

// In a browser, the app attaches to a Neuron host (`bun run host --mock` in
// ui/neuron-client). Its URL comes from NEURON_HOST_URL and its pairing token
// from the file NEURON_HOST_TOKEN_FILE names, so the token is never typed or
// printed. Inside Tauri neither is used.
// Only the dev server gets them: a production build never embeds the pairing token.
function devHost(command: 'build' | 'serve'): { url: string; token: string } {
  if (command !== 'serve') return { url: '', token: '' }
  const tokenFile = process.env.NEURON_HOST_TOKEN_FILE
  const token = tokenFile && fs.existsSync(tokenFile) ? fs.readFileSync(tokenFile, 'utf8').trim() : ''
  return { url: process.env.NEURON_HOST_URL ?? '', token }
}

// The updater only works with a public key (tauri.conf.json); without one the app links to the releases page.
const tauriConf = JSON.parse(fs.readFileSync(path.resolve(import.meta.dirname, 'src-tauri/tauri.conf.json'), 'utf8'))
const updaterSigned = isUpdaterSigned(tauriConf)

export default defineConfig(({ command }) => ({
  plugins: [react()],
  clearScreen: false,
  resolve: {
    alias: { '@neuron/client': path.resolve(import.meta.dirname, '../neuron-client/src/index.ts') },
  },
  define: {
    'import.meta.env.VITE_NEURON_HOST_URL': JSON.stringify(devHost(command).url),
    'import.meta.env.VITE_NEURON_HOST_TOKEN': JSON.stringify(devHost(command).token),
    'import.meta.env.VITE_NEURON_UPDATER_SIGNED': JSON.stringify(String(updaterSigned)),
  },
  server: {
    port: Number(process.env.NEURON_DESKTOP_PORT ?? 1420),
    strictPort: true,
    host: '127.0.0.1',
    fs: { allow: [path.resolve(import.meta.dirname, '..')] },
  },
  build: { target: 'es2022', outDir: 'dist', emptyOutDir: true },
}))
