import { describe, expect, it } from 'vitest'
import { absoluteApiUrl, curlExample, formatRemaining } from './apiToken'

describe('absoluteApiUrl', () => {
  it('resolves a relative path against the origin', () => {
    expect(absoluteApiUrl('/api', 'https://d123.cloudfront.net')).toBe('https://d123.cloudfront.net/api')
  })

  it('keeps an absolute URL and strips trailing slashes', () => {
    expect(absoluteApiUrl('http://localhost:3000/api/', 'http://localhost:5173')).toBe(
      'http://localhost:3000/api',
    )
  })
})

describe('curlExample', () => {
  it('uses the API base and a $HADES_TOKEN placeholder, never a real token', () => {
    const snippet = curlExample('https://example.test/api')
    expect(snippet).toContain('"https://example.test/api/journal"')
    expect(snippet).toContain('Authorization: Bearer $HADES_TOKEN')
    expect(snippet).toContain('-X PUT')
  })
})

describe('formatRemaining', () => {
  const now = 1_000_000_000_000
  it.each([
    [-1, 'expired'],
    [0, 'expired'],
    [30_000, 'in less than a minute'],
    [60_000, 'in 1 minute'],
    [54 * 60_000 + 5_000, 'in 54 minutes'],
    [90 * 60_000, 'in 1h 30m'],
  ])('%i ms -> %s', (delta, expected) => {
    expect(formatRemaining(now + delta, now)).toBe(expected)
  })
})
