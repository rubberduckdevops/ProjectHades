import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@cloudscape-design/global-styles', () => ({ applyMode: vi.fn(), Mode: { Light: 'light', Dark: 'dark' } }))

const store = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => void store.set(k, v),
})

const { DEFAULT_SETTINGS, loadSettings, saveSettings } = await import('./settings')

describe('settings persistence', () => {
  beforeEach(() => store.clear())

  it('returns defaults when nothing is stored', () => {
    expect(loadSettings()).toEqual(DEFAULT_SETTINGS)
  })

  it('round-trips saved settings', () => {
    saveSettings({ theme: 'dark', timeZone: 'Europe/Berlin' })
    expect(loadSettings()).toEqual({ theme: 'dark', timeZone: 'Europe/Berlin' })
  })

  it('sanitizes invalid stored values', () => {
    store.set('hades.settings', JSON.stringify({ theme: 'purple', timeZone: 'Not/AZone' }))
    expect(loadSettings()).toEqual(DEFAULT_SETTINGS)
    store.set('hades.settings', '{not json')
    expect(loadSettings()).toEqual(DEFAULT_SETTINGS)
  })
})
