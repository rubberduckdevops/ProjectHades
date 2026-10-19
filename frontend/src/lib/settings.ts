import { applyMode, Mode } from '@cloudscape-design/global-styles'
import { browserTimeZone, isValidTimeZone } from './dates'

export type ThemePreference = 'system' | 'light' | 'dark'

export interface Settings {
  theme: ThemePreference
  /** IANA timezone; null means "use the browser's timezone". */
  timeZone: string | null
}

export const DEFAULT_SETTINGS: Settings = { theme: 'system', timeZone: null }

const STORAGE_KEY = 'hades.settings'

/** Read settings from localStorage, falling back to defaults on any problem. */
export function loadSettings(): Settings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return DEFAULT_SETTINGS
    const parsed = JSON.parse(raw) as Partial<Settings>
    const theme: ThemePreference =
      parsed.theme === 'light' || parsed.theme === 'dark' ? parsed.theme : 'system'
    const timeZone =
      typeof parsed.timeZone === 'string' && isValidTimeZone(parsed.timeZone) ? parsed.timeZone : null
    return { theme, timeZone }
  } catch {
    return DEFAULT_SETTINGS
  }
}

export function saveSettings(settings: Settings): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(settings))
  } catch {
    // Storage unavailable (private mode, quota): settings just won't persist.
  }
}

/** The timezone to actually use for dates. */
export function effectiveTimeZone(settings: Settings): string {
  return settings.timeZone ?? browserTimeZone()
}

const darkQuery = () => window.matchMedia?.('(prefers-color-scheme: dark)')

export function resolveTheme(theme: ThemePreference): 'light' | 'dark' {
  if (theme === 'system') return darkQuery()?.matches ? 'dark' : 'light'
  return theme
}

/** Apply the theme to Cloudscape and the document. */
export function applyTheme(theme: ThemePreference): 'light' | 'dark' {
  const resolved = resolveTheme(theme)
  applyMode(resolved === 'dark' ? Mode.Dark : Mode.Light)
  document.documentElement.style.colorScheme = resolved
  return resolved
}

/** Call `onChange` when the OS color scheme changes. Returns an unsubscribe fn. */
export function watchSystemTheme(onChange: () => void): () => void {
  const mq = darkQuery()
  if (!mq) return () => {}
  mq.addEventListener('change', onChange)
  return () => mq.removeEventListener('change', onChange)
}
