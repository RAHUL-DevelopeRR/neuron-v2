/** Whether tauri.conf.json carries an updater public key; blank counts as none. */
export function isUpdaterSigned(conf: { plugins?: { updater?: { pubkey?: unknown } } }): boolean {
  const key = conf.plugins?.updater?.pubkey
  return typeof key === 'string' && key.trim().length > 0
}
