export interface DateRange {
  from: string
  to: string
}

/** The browser's IANA timezone, e.g. "America/Los_Angeles". */
export function browserTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone
}

/** All IANA timezones the runtime knows about. */
export function supportedTimeZones(): string[] {
  try {
    return Intl.supportedValuesOf('timeZone')
  } catch {
    return [browserTimeZone(), 'UTC']
  }
}

export function isValidTimeZone(timeZone: string): boolean {
  try {
    new Intl.DateTimeFormat('en-US', { timeZone })
    return true
  } catch {
    return false
  }
}

interface ZonedParts {
  year: number
  month: number // 1-12
  day: number
  hour: number
  minute: number
  second: number
}

const partsFormatters = new Map<string, Intl.DateTimeFormat>()

function zonedParts(date: Date, timeZone: string): ZonedParts {
  let fmt = partsFormatters.get(timeZone)
  if (!fmt) {
    fmt = new Intl.DateTimeFormat('en-US', {
      timeZone,
      hourCycle: 'h23',
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    })
    partsFormatters.set(timeZone, fmt)
  }
  const out: Record<string, number> = {}
  for (const p of fmt.formatToParts(date)) {
    if (p.type !== 'literal') out[p.type] = Number(p.value)
  }
  return out as unknown as ZonedParts
}

/** Offset of `timeZone` from UTC at instant `date`, in ms (e.g. -7h for PDT). */
function tzOffsetMs(date: Date, timeZone: string): number {
  const p = zonedParts(date, timeZone)
  const asUtc = Date.UTC(p.year, p.month - 1, p.day, p.hour, p.minute, p.second)
  return asUtc - Math.floor(date.getTime() / 1000) * 1000
}

/** The UTC instant of 00:00 on the given calendar day in `timeZone`. */
function zonedMidnight(year: number, month: number, day: number, timeZone: string): Date {
  const naive = Date.UTC(year, month - 1, day)
  // Two passes settle DST transitions where the offset differs at the guess.
  let guess = naive - tzOffsetMs(new Date(naive), timeZone)
  guess = naive - tzOffsetMs(new Date(guess), timeZone)
  return new Date(guess)
}

const pad = (n: number) => String(n).padStart(2, '0')

/** Whether `s` is a real calendar date written as `YYYY-MM-DD`. */
export function isCalendarDate(s: string): boolean {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s)
  if (!m) return false
  const d = new Date(Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3])))
  return d.getUTCFullYear() === Number(m[1]) && d.getUTCMonth() === Number(m[2]) - 1 && d.getUTCDate() === Number(m[3])
}

/** The calendar day `n` days after `day` (both `YYYY-MM-DD`; `n` may be negative). */
export function addDays(day: string, n: number): string {
  const [y, m, d] = day.split('-').map(Number)
  const date = new Date(Date.UTC(y, m - 1, d + n))
  return `${date.getUTCFullYear()}-${pad(date.getUTCMonth() + 1)}-${pad(date.getUTCDate())}`
}

/** Midnight to 23:59:59.999 of calendar day `day` (`YYYY-MM-DD`) in `timeZone`, as UTC ISO strings. */
export function dayRange(day: string, timeZone: string = browserTimeZone()): DateRange {
  const [year, month, date] = day.split('-').map(Number)
  const next = addDays(day, 1).split('-').map(Number)
  const start = zonedMidnight(year, month, date, timeZone)
  const end = zonedMidnight(next[0], next[1], next[2], timeZone)
  return { from: start.toISOString(), to: new Date(end.getTime() - 1).toISOString() }
}

/**
 * Midnight to 23:59:59.999 of `now`'s calendar day in `timeZone`
 * (defaults to the browser's), as UTC ISO strings.
 */
export function todayRange(now: Date = new Date(), timeZone: string = browserTimeZone()): DateRange {
  return dayRange(calendarDate(now, timeZone), timeZone)
}

/** `now`'s calendar day in `timeZone` as `YYYY-MM-DD`. */
export function calendarDate(now: Date = new Date(), timeZone: string = browserTimeZone()): string {
  const { year, month, day } = zonedParts(now, timeZone)
  return `${year}-${pad(month)}-${pad(day)}`
}

/** Format an ISO timestamp as HH:mm:ss in `timeZone` (defaults to the browser's). */
export function formatTime(iso: string, timeZone: string = browserTimeZone()): string {
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return iso
  const p = zonedParts(date, timeZone)
  return `${pad(p.hour)}:${pad(p.minute)}:${pad(p.second)}`
}

/** Long date label, e.g. "Monday, October 19, 2026", in `timeZone`. */
export function formatLongDate(date: Date, timeZone: string = browserTimeZone()): string {
  return date.toLocaleDateString(undefined, {
    timeZone,
    weekday: 'long',
    year: 'numeric',
    month: 'long',
    day: 'numeric',
  })
}


/** Short date and time, e.g. "Oct 19, 2026, 3:00 PM", in `timeZone`. */
export function formatDateTime(iso: string, timeZone: string = browserTimeZone()): string {
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return iso
  return date.toLocaleString(undefined, {
    timeZone,
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  })
}
