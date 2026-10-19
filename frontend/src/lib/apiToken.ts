/** Resolve a possibly-relative API URL (e.g. "/api") against the page origin. */
export function absoluteApiUrl(apiUrl: string, origin: string): string {
  try {
    return new URL(apiUrl, origin).toString().replace(/\/+$/, '')
  } catch {
    return apiUrl
  }
}

/** Shell snippet showing how to call the API with a token held in $HADES_TOKEN. */
export function curlExample(apiBase: string): string {
  return [
    "export HADES_TOKEN='<paste token here>'",
    '',
    '# List today\'s entries (UTC day)',
    `curl -H "Authorization: Bearer $HADES_TOKEN" \\`,
    `  "${apiBase}/journal"`,
    '',
    '# Add an entry',
    `curl -X PUT -H "Authorization: Bearer $HADES_TOKEN" \\`,
    `  -H "Content-Type: application/json" \\`,
    `  -d '{"body":"Hello from curl"}' \\`,
    `  "${apiBase}/journal"`,
  ].join('\n')
}

/** Human-friendly time remaining, e.g. "in 54 minutes", or "expired". */
export function formatRemaining(expiresAtMs: number, nowMs: number): string {
  const diff = expiresAtMs - nowMs
  if (diff <= 0) return 'expired'
  const minutes = Math.floor(diff / 60_000)
  if (minutes < 1) return 'in less than a minute'
  if (minutes < 60) return `in ${minutes} minute${minutes === 1 ? '' : 's'}`
  const hours = Math.floor(minutes / 60)
  const rem = minutes % 60
  return `in ${hours}h ${rem}m`
}
