/** Pages on zero-x.live the app opens in the browser. One place, so the gateway's routes can move. */

const SITE = 'https://zero-x.live'

export const LINKS = {
  pricing: `${SITE}/pricing?from=desktop`,
  apiKeys: `${SITE}/dashboard#api-keys`,
  account: `${SITE}/dashboard`,
  download: `${SITE}/download`,
  help: `${SITE}/docs`,
  providerKeys: `${SITE}/docs/providers`,
  releases: 'https://github.com/RAHUL-DevelopeRR/neuron-v2/releases',
  changelog: `${SITE}/changelog`,
} as const

/** Where a link really goes (host and path), for a `title`: link text can say anything. */
export function linkTitle(url: string): string {
  try {
    const parsed = new URL(url)
    return `${parsed.host}${parsed.pathname === '/' ? '' : parsed.pathname}` || url
  } catch {
    return url
  }
}
