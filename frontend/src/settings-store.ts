import { createContext, useContext } from 'react'
import type { Settings } from './lib/settings'

export interface SettingsContextValue {
  settings: Settings
  updateSettings: (patch: Partial<Settings>) => void
  /** Timezone in effect (the chosen one, or the browser's). */
  timeZone: string
  /** Theme in effect after resolving "system". */
  resolvedTheme: 'light' | 'dark'
}

export const SettingsContext = createContext<SettingsContextValue | null>(null)

export function useSettings(): SettingsContextValue {
  const ctx = useContext(SettingsContext)
  if (!ctx) throw new Error('useSettings must be used inside <SettingsProvider>')
  return ctx
}
