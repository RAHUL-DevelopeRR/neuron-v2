/** The normalised URL when it may be opened outside the app (`http:` or `https:` only), else `null`. */
export function safeExternalUrl(url: string): string | null {
  try {
    const parsed = new URL(url)
    return parsed.protocol === 'http:' || parsed.protocol === 'https:' ? parsed.href : null
  } catch {
    return null
  }
}
