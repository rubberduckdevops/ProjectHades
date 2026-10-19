import { useCallback, useEffect, useMemo, useState, useSyncExternalStore, type ReactNode } from 'react'
import {
  applyTheme,
  effectiveTimeZone,
  loadSettings,
  resolveTheme,
  saveSettings,
  watchSystemTheme,
  type Settings,
} from './lib/settings'
import { SettingsContext } from './settings-store'

const systemThemeSnapshot = () => resolveTheme('system')

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [settings, setSettings] = useState<Settings>(loadSettings)
  const systemTheme = useSyncExternalStore(watchSystemTheme, systemThemeSnapshot)
  const resolvedTheme = settings.theme === 'system' ? systemTheme : settings.theme

  useEffect(() => {
    applyTheme(resolvedTheme)
  }, [resolvedTheme])

  const updateSettings = useCallback((patch: Partial<Settings>) => {
    setSettings((prev) => {
      const next = { ...prev, ...patch }
      saveSettings(next)
      return next
    })
  }, [])

  const value = useMemo(
    () => ({ settings, updateSettings, timeZone: effectiveTimeZone(settings), resolvedTheme }),
    [settings, updateSettings, resolvedTheme],
  )

  return <SettingsContext.Provider value={value}>{children}</SettingsContext.Provider>
}
