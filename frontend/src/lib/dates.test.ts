import { describe, expect, it } from 'vitest'
import { formatTime, todayRange } from './dates'

describe('todayRange', () => {
  it('returns from < to as ISO strings', () => {
    const { from, to } = todayRange(new Date(2026, 9, 19, 14, 3, 22))
    expect(from).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/)
    expect(to).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/)
    expect(new Date(from).getTime()).toBeLessThan(new Date(to).getTime())
  })

  it('spans the local day', () => {
    const now = new Date(2026, 9, 19, 14, 3, 22)
    const { from, to } = todayRange(now)
    const start = new Date(from)
    const end = new Date(to)
    expect([start.getFullYear(), start.getMonth(), start.getDate()]).toEqual([2026, 9, 19])
    expect([start.getHours(), start.getMinutes(), start.getSeconds(), start.getMilliseconds()]).toEqual([0, 0, 0, 0])
    expect([end.getFullYear(), end.getMonth(), end.getDate()]).toEqual([2026, 9, 19])
    expect([end.getHours(), end.getMinutes(), end.getSeconds(), end.getMilliseconds()]).toEqual([23, 59, 59, 999])
    expect(start.getTime()).toBeLessThanOrEqual(now.getTime())
    expect(end.getTime()).toBeGreaterThanOrEqual(now.getTime())
  })
})

describe('formatTime', () => {
  it('formats as local HH:mm:ss', () => {
    const iso = new Date(2026, 9, 19, 9, 5, 7, 123).toISOString()
    expect(formatTime(iso)).toBe('09:05:07')
  })

  it('always produces HH:mm:ss', () => {
    expect(formatTime('2026-10-19T14:03:22.123Z')).toMatch(/^\d{2}:\d{2}:\d{2}$/)
  })
})

describe('timezone-aware helpers', () => {
  // 2026-10-19 03:30 UTC is still Oct 18 in Los Angeles (PDT, UTC-7).
  const now = new Date('2026-10-19T03:30:00.000Z')

  it('computes "today" in the given timezone', () => {
    expect(todayRange(now, 'America/Los_Angeles')).toEqual({
      from: '2026-10-18T07:00:00.000Z',
      to: '2026-10-19T06:59:59.999Z',
    })
    expect(todayRange(now, 'UTC')).toEqual({
      from: '2026-10-19T00:00:00.000Z',
      to: '2026-10-19T23:59:59.999Z',
    })
    expect(todayRange(now, 'Asia/Kolkata')).toEqual({
      from: '2026-10-18T18:30:00.000Z',
      to: '2026-10-19T18:29:59.999Z',
    })
  })

  it('handles DST transition days (25-hour and 23-hour days)', () => {
    // US fall back: 2026-11-01 in New York is 25 hours long.
    expect(todayRange(new Date('2026-11-01T12:00:00Z'), 'America/New_York')).toEqual({
      from: '2026-11-01T04:00:00.000Z',
      to: '2026-11-02T04:59:59.999Z',
    })
    // US spring forward: 2026-03-08 in New York is 23 hours long.
    expect(todayRange(new Date('2026-03-08T12:00:00Z'), 'America/New_York')).toEqual({
      from: '2026-03-08T05:00:00.000Z',
      to: '2026-03-09T03:59:59.999Z',
    })
  })

  it('formats times in the given timezone', () => {
    expect(formatTime('2026-10-19T03:30:05.000Z', 'America/Los_Angeles')).toBe('20:30:05')
    expect(formatTime('2026-10-19T03:30:05.000Z', 'UTC')).toBe('03:30:05')
  })
})
