/** Pages on zero-x.live the app opens in the browser. One place, so the gateway's routes can move. */

const SITE = 'https://zero-x.live'

export const LINKS = {
  pricing: `${SITE}/pricing?from=desktop`,
  apiKeys: `${SITE}/dashboard#api-keys`,
  account: `${SITE}/dashboard`,
  download: `${SITE}/download`,
  help: `${SITE}/docs`,
  providerKeys: `${SITE}/docs/providers`,
  changelog: `${SITE}/changelog`,
} as const
